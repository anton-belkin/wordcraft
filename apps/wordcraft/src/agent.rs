//! fork:agent — starts the agent layer: the `agent.*` commands, the Agent pane and the in-app MCP
//! server (`http://127.0.0.1:7982/mcp`), which reaches the document through the control channel.
//!
//! `--agent-port <port>` (or `WORDCRAFT_AGENT_PORT`) picks the port, `--no-agent` turns it off,
//! `--agent-demo` opens the demo template, `--agent-stdio` serves MCP over stdio instead of
//! opening a window (what Claude Desktop starts; see [`stdio_if_asked`]). With the agent on, the control channel starts too (on
//! 7981 unless `--control` says otherwise).

/// Control port used for the agent when none was asked for.
const CONTROL_PORT: u16 = 7981;

/// The agent's own command-line options.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub port: Option<u16>,
    pub demo: bool,
}

/// Takes the agent's own arguments out of the command line; returns the rest.
pub fn take_args(args: Vec<String>) -> (Vec<String>, Options) {
    let mut o = Options {
        port: std::env::var("WORDCRAFT_AGENT_PORT").ok().and_then(|p| p.parse().ok()).or(Some(wordcraft_agent::server::DEFAULT_PORT)),
        demo: false,
    };
    let mut rest = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--agent-port" => o.port = it.next().and_then(|p| p.parse().ok()),
            "--no-agent" => o.port = None,
            "--agent-demo" => o.demo = true,
            _ => rest.push(a),
        }
    }
    (rest, o)
}

/// The control port: the agent server needs one.
pub fn control_port(o: Options, control: Option<u16>) -> Option<u16> {
    if o.port.is_some() { control.or(Some(CONTROL_PORT)) } else { control }
}

/// Registers the agent layer (before the session is made: its registry is built then).
pub fn install(o: Options) {
    if o.port.is_some() {
        wordcraft_agent_ui::install();
    }
}

/// The demo template, when asked for.
pub fn demo_document(o: Options) -> Option<wordcraft_doc::Document> {
    (o.demo && o.port.is_some()).then(wordcraft_agent::sample::engagement_letter)
}

/// Starts the agent server once the control channel listens.
pub fn start(o: Options, control: Option<u16>) {
    let (Some(port), Some(control)) = (o.port, control) else { return };
    match wordcraft_agent::server::start(port, wordcraft_agent::server::control(format!("127.0.0.1:{control}"))) {
        Ok(port) => {
            wordcraft_agent_ui::set_server_url(format!("http://127.0.0.1:{port}/mcp"));
            if let Some(e) = stdio_entry(Some(control)) {
                wordcraft_agent_ui::set_stdio_entry(e);
            }
        }
        Err(e) => log::error!("agent server on port {port}: {e}"),
    }
}

/// `wordcraft --agent-stdio [--control HOST:PORT]`: serve the agent layer's MCP over stdio (for
/// Claude Desktop and Cowork), reaching the running app through its control channel, and exit.
/// Nothing else of the app starts, and nothing but protocol goes to stdout.
pub fn stdio_if_asked() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == wordcraft_agent::cowork::STDIO_FLAG) {
        return;
    }
    let port = std::env::var("WORDCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(CONTROL_PORT);
    let mut addr = format!("127.0.0.1:{port}");
    if let Some(v) = args.iter().position(|a| a == "--control").and_then(|i| args.get(i + 1)) {
        addr = if v.contains(':') { v.clone() } else { format!("127.0.0.1:{v}") };
    }
    let mut backend = wordcraft_agent::stdio::Lazy::new(wordcraft_agent::server::control(addr));
    let code = match wordcraft_agent::stdio::serve(&mut backend, std::io::stdin().lock(), std::io::stdout()) {
        Ok(()) => 0,
        Err(e) => {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("wordcraft --agent-stdio: {e}\n"));
            1
        }
    };
    std::process::exit(code);
}

/// The command Claude Desktop should start to reach this app (this very executable in stdio mode).
pub fn stdio_entry(control: Option<u16>) -> Option<serde_json::Value> {
    let exe = std::env::current_exe().ok()?;
    Some(wordcraft_agent::cowork::entry(&exe, control.unwrap_or(CONTROL_PORT)))
}
