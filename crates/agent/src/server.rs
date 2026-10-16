//! The in-app agent server: HTTP on 127.0.0.1, started by the app itself, so connecting an agent
//! is one line of MCP config (`http://127.0.0.1:7982/mcp`) and no extra process.
//!
//! - `POST /mcp` — MCP over Streamable HTTP (JSON responses; no server-initiated stream).
//! - `GET /events` — one JSON line per new question, kept open: what an agent's background
//!   monitor watches (`curl -sN http://127.0.0.1:7982/events`).
//! - `GET /health` — `ok`.
//!
//! It reaches the document through a [`Connector`]: the app's loopback control channel
//! ([`control`]), or anything else that answers control requests (the offscreen demo).

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use wordcraft_mcp::{Backend, Remote};

/// Default port of the agent server (GridCraft's is 7980, WordCraft's control channel 7981).
pub const DEFAULT_PORT: u16 = 7982;
/// Largest request body accepted.
const MAX_BODY: usize = 16 * 1024 * 1024;
/// Largest request head accepted.
const MAX_HEAD: usize = 64 * 1024;
/// Most connections served at once.
const MAX_CONNS: usize = 64;

/// Makes a connection to the document (one per HTTP connection).
pub type Connector = Arc<dyn Fn() -> Result<Box<dyn Backend + Send>, String> + Send + Sync>;

/// A connector to the app's control channel at `addr` (`127.0.0.1:7981`).
pub fn control(addr: String) -> Connector {
    Arc::new(move || {
        Remote::connect(&addr)
            .map(|r| Box::new(r) as Box<dyn Backend + Send>)
            .map_err(|e| format!("WordCraft control channel at {addr} is not reachable: {e}"))
    })
}

/// Starts the server on `port` (0 = any). Returns the port it listens on.
pub fn start(port: u16, connect: Connector) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::Builder::new().name("agent-server".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            if live.load(std::sync::atomic::Ordering::Relaxed) >= MAX_CONNS {
                continue;
            }
            let connect = connect.clone();
            let live = live.clone();
            live.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let spawned = std::thread::Builder::new().name("agent-conn".into()).spawn({
                let live = live.clone();
                move || {
                    serve(stream, &connect);
                    live.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
            if spawned.is_err() {
                live.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    })?;
    log::info!("agent MCP server on http://127.0.0.1:{port}/mcp");
    Ok(port)
}

struct Request {
    method: String,
    path: String,
    /// The query string (after `?`), undecoded.
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn read_request(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = r.by_ref().take(MAX_HEAD as u64).read_until(b'\n', &mut line).ok()?;
        if n == 0 {
            return None;
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD {
            return None;
        }
    }
    let head = String::from_utf8(head).ok()?;
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.to_string(), query.to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return None;
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    Some(Request { method, path, query, headers, body })
}

fn respond(w: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(w, "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()
}

/// Browsers may only reach us from pages on this machine (DNS-rebinding guard, as MCP asks).
fn origin_ok(req: &Request) -> bool {
    match req.header("origin") {
        None => true,
        Some(o) => {
            let host = o.split("://").nth(1).unwrap_or(o);
            let host = if host.starts_with('[') {
                host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
            } else {
                host.split(':').next().unwrap_or("").to_string()
            };
            matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") || o == "null"
        }
    }
}

fn serve(stream: TcpStream, connect: &Connector) {
    let _ = stream.set_nodelay(true);
    let Ok(mut w) = stream.try_clone() else { return };
    let mut r = BufReader::new(stream);
    let mut backend: Option<Box<dyn Backend + Send>> = None;
    while let Some(req) = read_request(&mut r) {
        if !origin_ok(&req) {
            let _ = respond(&mut w, "403 Forbidden", "text/plain", b"origin not allowed");
            return;
        }
        let result = match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/mcp") => post_mcp(&mut w, &req, connect, &mut backend),
            ("GET", "/mcp") => {
                channel_stream(w, connect);
                return;
            }
            ("DELETE", "/mcp") => respond(&mut w, "200 OK", "text/plain", b""),
            ("GET", "/events") => {
                events(w, connect, query_param(&req.query, "document"));
                return;
            }
            ("GET", "/health") => respond(&mut w, "200 OK", "text/plain", b"ok"),
            ("GET", "/") => respond(&mut w, "200 OK", "text/plain", HELP.as_bytes()),
            _ => respond(&mut w, "404 Not Found", "text/plain", b"not found"),
        };
        if result.is_err() || req.header("connection").is_some_and(|c| c.eq_ignore_ascii_case("close")) {
            return;
        }
    }
}

const HELP: &str = "WordCraft agent layer\n\nPOST /mcp     MCP (Streamable HTTP)\nGET  /mcp     questions as channel events (SSE, in the \"my session\" mode)\nGET  /events  one JSON line per new question (for a background monitor)\nGET  /health  ok\n";

fn backend<'a>(slot: &'a mut Option<Box<dyn Backend + Send>>, connect: &Connector) -> Result<&'a mut Box<dyn Backend + Send>, String> {
    if slot.is_none() {
        *slot = Some(connect()?);
    }
    slot.as_mut().ok_or_else(|| "not connected".to_string())
}

fn post_mcp(w: &mut TcpStream, req: &Request, connect: &Connector, slot: &mut Option<Box<dyn Backend + Send>>) -> std::io::Result<()> {
    let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
        let e = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}});
        return respond(w, "400 Bad Request", "application/json", e.to_string().as_bytes());
    };
    let reply = match backend(slot, connect) {
        Ok(b) => handle_any(b.as_mut(), &msg),
        Err(e) => msg.get("id").map(|id| json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": e}})),
    };
    match reply {
        Some(v) => respond(w, "200 OK", "application/json", v.to_string().as_bytes()),
        None => respond(w, "202 Accepted", "text/plain", b""),
    }
}

/// One JSON-RPC message or a batch.
pub fn handle_any(b: &mut dyn Backend, msg: &Value) -> Option<Value> {
    match msg {
        Value::Array(batch) => {
            let out: Vec<Value> = batch.iter().take(100).filter_map(|m| crate::mcp::handle(b, m)).collect();
            (!out.is_empty()).then_some(Value::Array(out))
        }
        m => crate::mcp::handle(b, m),
    }
}

/// The event line for a pending question (masked: it comes from `agent.questions`).
pub fn event_line(q: &Value) -> Option<Value> {
    let id = q.get("id").and_then(Value::as_str)?;
    Some(json!({
        "event": "question",
        "id": id,
        "paragraph": q.get("paragraph"),
        "quote": q.get("quote"),
        "question": q.get("text"),
        "kind": q.get("kind").and_then(Value::as_str).unwrap_or("question"),
        "topic": q.get("topic"),
        "document": q.get("document"),
        "documentTitle": q.get("documentTitle"),
        "hint": if q.get("kind").and_then(Value::as_str) == Some("ping") {
            "a connection check: answer_question with \"here\" at once"
        } else {
            "claim_question, investigate (get_context, read_document), answer_question"
        },
    }))
}

/// Streams new pending questions, one JSON object per line, until the client goes away.
/// A query parameter, percent-decoded.
pub fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| percent_decode(v))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| b.get(j).and_then(|x| (*x as char).to_digit(16));
        match (c, hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b'+', _, _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a question belongs on a stream filtered to `document`.
pub fn for_document(q: &Value, document: Option<&str>) -> bool {
    document.is_none_or(|d| q.get("document").and_then(Value::as_str) == Some(d))
}

fn events(mut w: TcpStream, connect: &Connector, document: Option<String>) {
    let _listening = crate::presence::Listening::new(crate::presence::Stream::Events);
    if w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").is_err() {
        return;
    }
    let _ = w.flush();
    let mut seen: HashSet<String> = HashSet::new();
    let mut slot: Option<Box<dyn Backend + Send>> = None;
    loop {
        if let Ok(b) = backend(&mut slot, connect) {
            match b.call(
                "engine.execute",
                json!({"command": "agent.questions", "params": {"status": "pending", "actor": "agent", "masked": true, "poll": true}}),
            ) {
                Ok(Value::Array(list)) => {
                    for q in list.into_iter().filter(|q| for_document(q, document.as_deref())) {
                        let Some(line) = event_line(&q) else { continue };
                        // A question sent again (a redo after an undo) is a new event.
                        let key = format!(
                            "{}@{}",
                            line.get("id").and_then(Value::as_str).unwrap_or(""),
                            q.get("askedAt").and_then(Value::as_u64).unwrap_or(0)
                        );
                        if !seen.insert(key) {
                            continue;
                        }
                        if writeln!(w, "{line}").and_then(|()| w.flush()).is_err() {
                            return;
                        }
                    }
                    // Questions the person took back (undo) while the agent had them.
                    if let Ok(Value::Array(gone)) = b.call(
                        "engine.execute",
                        json!({"command": "agent.questions", "params": {"status": "withdrawn", "actor": "agent", "masked": true, "poll": true}}),
                    ) {
                        for q in gone.into_iter().filter(|q| for_document(q, document.as_deref())) {
                            let id = q.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                            let key = format!("withdrawn:{id}@{}", q.get("askedAt").and_then(Value::as_u64).unwrap_or(0));
                            if !seen.insert(key) {
                                continue;
                            }
                            let line = json!({"event": "withdrawn", "id": id, "document": q.get("document"), "hint": "the user took this question back: drop it"});
                            if writeln!(w, "{line}").and_then(|()| w.flush()).is_err() {
                                return;
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => slot = None,
            }
        }
        std::thread::sleep(Duration::from_millis(800));
        if w.peer_addr().is_err() {
            return;
        }
    }
}

/// The `GET /mcp` stream (SSE): in the "my session" mode, each new pending question as a channel
/// event; questions that wait while the mode is another are sent when it is switched on.
fn channel_stream(mut w: TcpStream, connect: &Connector) {
    if w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n: connected\n\n")
        .and_then(|()| w.flush())
        .is_err()
    {
        return;
    }
    let _listening = crate::presence::Listening::new(crate::presence::Stream::Channel);
    let mut seen: HashSet<String> = HashSet::new();
    let mut slot: Option<Box<dyn Backend + Send>> = None;
    let mut quiet = 0u32;
    loop {
        if crate::presence::mode() == crate::presence::Mode::Channel
            && let Ok(b) = backend(&mut slot, connect)
        {
            match b.call(
                "engine.execute",
                json!({"command": "agent.questions", "params": {"status": "pending", "actor": "agent", "masked": true, "poll": true}}),
            ) {
                Ok(Value::Array(list)) => {
                    for q in list {
                        let Some(id) = q.get("id").and_then(Value::as_str).map(str::to_string) else { continue };
                        if !seen.insert(id) {
                            continue;
                        }
                        let event = format!("event: message\ndata: {}\n\n", crate::channel::notification(&q));
                        if w.write_all(event.as_bytes()).and_then(|()| w.flush()).is_err() {
                            return;
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => slot = None,
            }
        }
        std::thread::sleep(Duration::from_millis(800));
        quiet += 1;
        // A comment now and then notices a client that went away.
        if quiet >= 20 {
            quiet = 0;
            if w.write_all(b": keep-alive\n\n").and_then(|()| w.flush()).is_err() {
                return;
            }
        }
    }
}
