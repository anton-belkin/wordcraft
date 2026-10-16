//! "Send to my Claude Code session" (Claude Code Channels, research preview): the app pushes each
//! new question on its `GET /mcp` stream (SSE) as `notifications/claude/channel`, and the stdio
//! server ([`crate::stdio`]) passes them on to the Claude Code session that started it:
//!
//! ```text
//! claude mcp add wordcraft-channel -- /path/to/wordcraft --agent-stdio
//! claude --dangerously-load-development-channels server:wordcraft-channel
//! ```
//!
//! Interactive sessions only (`claude -p` ignores channels).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value as Json, json};

/// The server's name for the channel (`server:<name>` on Claude Code's command line).
pub const SERVER_NAME: &str = "wordcraft-channel";

/// The channel notification for a question (masked: it comes from `agent.questions`).
pub fn notification(q: &Json) -> Json {
    let get = |k: &str| q.get(k).and_then(Json::as_str).unwrap_or("").to_string();
    let id = get("id");
    let content = if get("kind") == "ping" {
        format!("Connection check from WordCraft: answer it at once with \"here\" (answer_question, id {id}).")
    } else {
        let para = q.get("paragraph").and_then(Json::as_u64).map(|n| format!("paragraph {n}")).unwrap_or_else(|| "the document".into());
        let quote = get("quote");
        let on = if quote.is_empty() { para.clone() } else { format!("{para} (“{quote}”)") };
        format!(
            "Question in WordCraft on {on}: {} — question id {id}. Claim it, investigate with the wordcraft tools, answer with answer_question.",
            get("text")
        )
    };
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": {
            "content": content,
            // Keys must be identifiers (letters, digits, underscores).
            "meta": {"question_id": id, "kind": if get("kind") == "ping" { "ping" } else { "question" }},
        },
    })
}

/// Follows the app's `GET /mcp` stream on `port` and writes each event to `out`; reconnects while
/// the app is away. Runs until the process ends.
pub fn follow(port: u16, out: Arc<Mutex<dyn Write + Send>>) {
    loop {
        if let Ok(mut c) = TcpStream::connect(("127.0.0.1", port)) {
            let req = format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/event-stream\r\n\r\n");
            if c.write_all(req.as_bytes()).is_ok() {
                for line in BufReader::new(c).lines().map_while(Result::ok) {
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let Ok(v) = serde_json::from_str::<Json>(data.trim()) else { continue };
                    let mut w = match out.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    if writeln!(w, "{v}").and_then(|()| w.flush()).is_err() {
                        return;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_and_checks_become_channel_events() {
        let q = json!({"id": "q3", "paragraph": 13, "quote": "payable within 30 days", "text": "Is that usual?"});
        let n = notification(&q);
        assert_eq!(n["method"], "notifications/claude/channel");
        assert!(n["params"]["content"].as_str().unwrap().contains("paragraph 13 (“payable within 30 days”): Is that usual?"));
        assert_eq!(n["params"]["meta"]["question_id"], "q3");
        let p = notification(&json!({"id": "ping1", "kind": "ping"}));
        assert_eq!(p["params"]["meta"]["kind"], "ping");
        assert!(p["params"]["content"].as_str().unwrap().contains("\"here\""));
    }
}
