//! fork:agent — starts the agent layer: the `agent.*` commands, the Agent pane and the in-app MCP
//! server (`http://127.0.0.1:7982/mcp`), which reaches the document through the control channel.
//!
//! `--agent-port <port>` (or `WORDCRAFT_AGENT_PORT`) picks the port, `--no-agent` turns it off,
//! `--agent-demo` opens the demo template. With the agent on, the control channel starts too (on
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
        Ok(port) => wordcraft_agent_ui::set_server_url(format!("http://127.0.0.1:{port}/mcp")),
        Err(e) => log::error!("agent server on port {port}: {e}"),
    }
}
