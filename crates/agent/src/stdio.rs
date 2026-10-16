//! The agent layer's MCP over stdio, for clients that start local servers as processes (Claude
//! Desktop and Cowork, `claude mcp add` without `--transport http`). The same tools as the in-app
//! HTTP server; the document is reached through the app's control channel, connected on first use,
//! so the client can start before WordCraft does and keeps working across app restarts.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wordcraft_mcp::Backend;

use crate::server::Connector;

/// A backend that connects on first use and again after the app went away.
pub struct Lazy {
    connect: Connector,
    slot: Option<Box<dyn Backend + Send>>,
}

impl Lazy {
    pub fn new(connect: Connector) -> Self {
        Lazy { connect, slot: None }
    }
}

impl Backend for Lazy {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.slot.is_none() {
            let b = (self.connect)().map_err(|e| format!("{e}. Is WordCraft open (with the agent layer on)?"))?;
            self.slot = Some(b);
        }
        let Some(b) = self.slot.as_mut() else { return Err("not connected".into()) };
        let r = b.call(method, params);
        if let Err(e) = &r
            && e.contains("not reachable")
        {
            self.slot = None;
        }
        r
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "the WordCraft app (agent layer)".into()
    }
}

/// Serves newline-delimited JSON-RPC from `input` until it closes. Nothing but protocol goes to
/// `output`; replies and channel events written through it never interleave.
pub fn serve(b: &mut dyn Backend, input: impl BufRead, output: Arc<Mutex<dyn Write + Send>>) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => crate::server::handle_any(b, &msg),
            Err(_) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})),
        };
        if let Some(v) = reply {
            let mut w = match output.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            writeln!(w, "{v}")?;
            w.flush()?;
        }
    }
    Ok(())
}

/// `--agent-stdio`: serves on stdin/stdout until stdin closes, reaching the app's control channel at
/// `control`, and passes on the channel events of its agent server on `agent_port` (the "my
/// session" mode). Returns the exit code.
pub fn run(control: String, agent_port: u16) -> i32 {
    let out: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(std::io::stdout()));
    let follower = out.clone();
    let _ = std::thread::Builder::new().name("channel-follow".into()).spawn(move || crate::channel::follow(agent_port, follower));
    let mut b = Lazy::new(crate::server::control(control));
    match serve(&mut b, std::io::stdin().lock(), out) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "wordcraft --agent-stdio: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_without_the_app_and_says_why_a_tool_fails() {
        let connect: Connector = Arc::new(|| Err("WordCraft control channel at 127.0.0.1:1 is not reachable".to_string()));
        let mut b = Lazy::new(connect);
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "not json",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_context","arguments":{}}}"#,
        ]
        .join("\n");
        let out = Arc::new(Mutex::new(Vec::new()));
        serve(&mut b, input.as_bytes(), out.clone()).unwrap();
        let lines: Vec<Value> = String::from_utf8(out.lock().unwrap().clone()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 4, "the notification gets no reply");
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "wordcraft-agent");
        assert!(lines[1]["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "wait_for_questions"));
        assert_eq!(lines[2]["error"]["code"], -32700);
        assert_eq!(lines[3]["result"]["isError"], true);
        assert!(lines[3]["result"]["content"][0]["text"].as_str().unwrap().contains("Is WordCraft open"));
    }

    #[test]
    fn tools_reach_the_document() {
        crate::install();
        let mut h = wordcraft_mcp::Headless::default();
        h.session.set_document(wordcraft_doc::Document::from_text("Hello there"));
        h.session.run("agent.ask", &json!({"paragraph": 0, "text": "Why hello?"})).unwrap();
        let input = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"wait_for_questions","arguments":{"timeout_seconds":0}}}"#;
        let out = Arc::new(Mutex::new(Vec::new()));
        serve(&mut h, input.as_bytes(), out.clone()).unwrap();
        assert!(String::from_utf8(out.lock().unwrap().clone()).unwrap().contains("Why hello?"));
    }
}
