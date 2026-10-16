//! Offscreen WordCraft with the agent layer: the whole window rendered with wgpu (works with a
//! locked screen), driven by a script, with the real in-app MCP server on a port so an agent (or
//! `curl`) can talk to it while it runs.
//!
//! `cargo run --release -p wordcraft-agent-ui --example agent_demo -- script.jsonl [shots-dir]`
//!
//! The script is JSON lines:
//! - `{"start": "demo" | "sample" | "blank", "width": 1440, "height": 900, "dark": false}` first;
//! - `{"serve": 7982}` starts the agent server, talking to this window;
//! - `{"cmd": "agent.ask", "params": {…}}` runs a command as the reader would;
//! - `{"method": "ui.inspect", "params": {…}}` a control-channel request;
//! - `{"exec": "curl -s …"}` runs a shell command while the window keeps running, prints its output;
//! - `{"pane": "questions"}` / `{"pane": null}` opens or closes the Agent pane;
//! - `{"menu": "context"}` / `{"menu": "ready"}` opens a menu of the Agent tab's Context group (`none` closes);
//! - `{"voice": {"secs": 4, "pointed": ["quote", …]}}` shows a recording (no microphone);
//! - `{"steps": 10}`, `{"shot": "/abs/out.png"}`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wordcraft_engine::Session;
use wordcraft_ui_egui::{ControlRequest, Services, WordApp};

/// Control requests through the window's channel (what the TCP control server does in the app).
struct Channel(Arc<Mutex<Sender<ControlRequest>>>);

impl wordcraft_mcp::Backend for Channel {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let (req, reply) = ControlRequest::new(method, params);
        self.0.lock().map_err(|_| "closed")?.send(req).map_err(|_| "window closed".to_string())?;
        let v = reply.recv_timeout(std::time::Duration::from_secs(30)).map_err(|_| "no reply".to_string())?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string())
        }
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "offscreen window".into()
    }
}

use wordcraft_agent::server as agent_server;

fn main() {
    let path = std::env::args().nth(1).expect("usage: agent_demo script.jsonl");
    // `SHOTS` in the script stands for the folder the screenshots go to (the second argument).
    let shots = std::env::args().nth(2).unwrap_or_else(|| std::env::temp_dir().to_string_lossy().trim_end_matches('/').to_string());
    let script = std::fs::read_to_string(&path).expect("read script").replace("SHOTS", &shots);
    let lines: Vec<Value> = script
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
        .map(|l| serde_json::from_str(l).expect("json line"))
        .collect();
    let first = lines.first().cloned().unwrap_or_default();
    wordcraft_agent_ui::install();
    let doc = match first.get("start").and_then(Value::as_str) {
        Some("sample") => wordcraft_engine::sample::sample_document(),
        Some("blank") => wordcraft_doc::Document::new(),
        _ => wordcraft_agent::sample::engagement_letter(),
    };
    let mut session = Session::new(doc);
    session.author = "Anton (reviewer)".into();
    if first.get("start").and_then(Value::as_str).is_none_or(|s| s == "demo") {
        session.file_name = Some("Engagement letter (template).docx".into());
    }
    let (tx, rx) = std::sync::mpsc::channel::<ControlRequest>();
    let tx = Arc::new(Mutex::new(tx));
    let mut app = WordApp::new(session, Services::default()).with_control(rx);
    app.integrated_titlebar = true;
    let dark = first.get("dark").and_then(Value::as_bool).unwrap_or(false);
    app.ui.theme = if dark { wordcraft_ui_egui::theme::Appearance::Dark } else { wordcraft_ui_egui::theme::Appearance::Light };
    let w = first.get("width").and_then(Value::as_f64).unwrap_or(1440.0) as f32;
    let h = first.get("height").and_then(Value::as_f64).unwrap_or(900.0) as f32;
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(w, h)).with_pixels_per_point(2.0).with_max_steps(1_000_000).wgpu().build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.input_mut().max_texture_side = Some(8192);
    step_n(&mut harness, 8);
    let mut last_id = String::new();
    for l in lines.iter().skip(1) {
        if let Some(port) = l.get("serve").and_then(Value::as_u64) {
            let tx = tx.clone();
            let connect: agent_server::Connector = Arc::new(move || Ok(Box::new(Channel(tx.clone())) as Box<dyn wordcraft_mcp::Backend + Send>));
            let port = agent_server::start(port as u16, connect).expect("agent server");
            wordcraft_agent_ui::set_server_url(format!("http://127.0.0.1:{port}/mcp"));
            // Connect Cowork shows the status for the app built next to this example.
            if let Some(app) = std::env::current_exe().ok().and_then(|e| e.parent().and_then(|d| d.parent()).map(|d| d.join("wordcraft"))) {
                wordcraft_agent_ui::set_stdio_entry(wordcraft_agent::cowork::entry(&app, 7981));
            }
            println!("agent server on http://127.0.0.1:{port}/mcp");
            // The AItoCraft registry, in the shots folder (not the user's).
            wordcraft_agent::registry::start(format!("http://127.0.0.1:{port}/mcp"), Some(std::path::PathBuf::from(&shots).join("registry")));
        } else if let Some(id) = l.get("cmd").and_then(Value::as_str) {
            // `$last`: the id the previous command returned.
            let raw = l.get("params").cloned().unwrap_or(json!({})).to_string().replace("\"$last\"", &json!(last_id).to_string());
            let params: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
            let r = harness.state_mut().run(id, params);
            if let Some(x) = r.as_ref().ok().and_then(|v| v.get("id")).and_then(Value::as_str) {
                last_id = x.to_string();
            }
            harness.state_mut().canvas.scroll_to_caret = true;
            println!("{id}: {}", short(&format!("{r:?}")));
            step_n(&mut harness, 3);
        } else if let Some(m) = l.get("method").and_then(Value::as_str) {
            let (req, reply) = ControlRequest::new(m, l.get("params").cloned().unwrap_or_default());
            tx.lock().unwrap().send(req).unwrap();
            for _ in 0..120 {
                step(&mut harness);
                if let Ok(r) = reply.try_recv() {
                    println!("{m}: {}", short(&r.to_string()));
                    break;
                }
            }
        } else if let Some(cmd) = l.get("exec").and_then(Value::as_str) {
            let child = std::process::Command::new("sh").arg("-c").arg(cmd).stdout(std::process::Stdio::piped()).spawn().expect("spawn");
            let out = std::thread::spawn(move || child.wait_with_output());
            while !out.is_finished() {
                step(&mut harness);
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
            let o = out.join().unwrap().unwrap();
            println!("$ {cmd}\n{}", String::from_utf8_lossy(&o.stdout).trim_end());
            step_n(&mut harness, 4);
        } else if let Some(p) = l.get("pane") {
            match p.as_str() {
                Some(tab) => wordcraft_agent_ui::request_pane(true, tab),
                None => wordcraft_agent_ui::request_pane(false, "ask"),
            }
            step_n(&mut harness, 4);
        } else if let Some(v) = l.get("voice") {
            // A recording shown without a microphone: {"secs": 4.0, "pointed": ["quote", …]}.
            let secs = v.get("secs").and_then(Value::as_f64).unwrap_or(3.0) as f32;
            let pointed: Vec<String> = v
                .get("pointed")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let downloading = v.get("downloading").and_then(Value::as_array).and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?)));
            wordcraft_agent_ui::voice::preview(wordcraft_agent_ui::voice::Preview {
                recording: v.get("pointed").map(|_| (secs, pointed)),
                downloading,
            });
            step_n(&mut harness, 4);
        } else if let Some(m) = l.get("menu").and_then(Value::as_str) {
            wordcraft_agent_ui::request_menu(m);
            step_n(&mut harness, 4);
        } else if let Some(n) = l.get("steps").and_then(Value::as_u64) {
            step_n(&mut harness, n as usize);
        } else if let Some(p) = l.get("shot").and_then(Value::as_str) {
            step_n(&mut harness, 6);
            match harness.render() {
                Ok(img) => {
                    img.save(p).expect("save png");
                    println!("shot {p}");
                }
                Err(e) => eprintln!("render failed: {e}"),
            }
        }
    }
}

fn short(s: &str) -> String {
    if s.len() > 400 { format!("{}…", s.get(..s.char_indices().nth(400).map_or(s.len(), |(i, _)| i)).unwrap_or(s)) } else { s.to_string() }
}

fn step(h: &mut egui_kittest::Harness<'_, WordApp>) {
    h.step();
}

fn step_n(h: &mut egui_kittest::Harness<'_, WordApp>, n: usize) {
    for _ in 0..n {
        step(h);
    }
}
