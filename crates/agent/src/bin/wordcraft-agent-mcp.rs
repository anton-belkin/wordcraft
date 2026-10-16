//! The WordCraft agent layer as a stdio MCP server, for clients that start local servers as
//! processes (Claude Desktop, Cowork). It talks to the running app through its control channel.
//!
//! `wordcraft-agent-mcp [--control 127.0.0.1:7981]` (or `WORDCRAFT_CONTROL_ADDR`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let mut addr = std::env::var("WORDCRAFT_CONTROL_ADDR").unwrap_or_else(|_| "127.0.0.1:7981".to_string());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => {
                if let Some(v) = args.next() {
                    addr = if v.contains(':') { v } else { format!("127.0.0.1:{v}") };
                }
            }
            "--help" | "-h" => {
                let _ = std::io::Write::write_all(
                    &mut std::io::stderr(),
                    b"wordcraft-agent-mcp [--control HOST:PORT]  (default 127.0.0.1:7981): the WordCraft agent layer over stdio MCP\n",
                );
                return;
            }
            _ => {}
        }
    }
    std::process::exit(wordcraft_agent::stdio::run(addr, wordcraft_agent::server::DEFAULT_PORT));
}

#[cfg(target_arch = "wasm32")]
fn main() {}
