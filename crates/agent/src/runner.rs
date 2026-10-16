//! The background agent ([`Mode::Background`]): the app runs Claude Code itself, one
//! `claude -p --input-format stream-json --output-format stream-json --verbose` process per
//! document, fed each new question once as a stream-json user message.
//!
//! - It runs in a neutral, empty folder of the app (`ArtCraft/agent-sessions/`, its sessions grouped
//!   there in Claude Desktop), so no project's CLAUDE.md, memory or settings become its context;
//!   the document's folder is an explicit option ([`Config::document_folder`]). Its session is
//!   named `WordCraft · <file>` (`--name`).
//! - It sees only the WordCraft server (`--mcp-config` + `--strict-mcp-config`, the user's own
//!   servers and plugins stay out of its context) and may use only its tools
//!   (`--allowedTools "mcp__wordcraft__*"`); the agent skill is its system prompt
//!   (`--append-system-prompt-file`); `--max-turns` and `--max-budget-usd` bound it.
//! - Its session id is kept in the layer (so in the `.docx`): reopening the document resumes it
//!   (`--resume`). A resume that fails here, or New session, starts a fresh one from a summary of
//!   the document's history.
//! - Stop is end of input, then SIGINT, then kill. Never `--bare` (it needs an API key; a
//!   subscription login works without one).
//!
//! The mode is the app's, never the file's: opening a document someone sent must not start an
//! agent that spends the reader's money.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value as Json, json};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::layer::{Actor, Layer, QuestionStatus};
pub use crate::presence::Mode;
use crate::presence::{self, store_mode};

// ---------------------------------------------------------------- the mode

pub fn mode() -> Mode {
    presence::mode()
}

/// Switches the mode (and remembers it for the next start, when a folder is configured).
pub fn set_mode(m: Mode) {
    store_mode(m);
    if let Some(dir) = config().data_dir {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("mode"), m.id());
    }
}

/// The mode remembered from the last run (the app's setting, not the document's).
pub fn load_mode() -> Option<Mode> {
    let dir = config().data_dir?;
    std::fs::read_to_string(dir.join("mode")).ok().and_then(|s| Mode::parse(&s))
}

/// The neutral folder background sessions run in: `ArtCraft/agent-sessions/`.
pub fn neutral_folder() -> Option<PathBuf> {
    crate::registry::folder().and_then(|d| d.parent().map(|p| p.join("agent-sessions")))
}

/// Whether the background agent runs in the document's folder (remembered, like the mode).
pub fn set_document_folder(on: bool) {
    let mut c = config();
    c.document_folder = on;
    if let Some(dir) = &c.data_dir {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("cwd"), if on { "document" } else { "neutral" });
    }
    set_config(c);
}

/// The folder choice remembered from the last run.
pub fn load_document_folder() -> bool {
    config().data_dir.and_then(|d| std::fs::read_to_string(d.join("cwd")).ok()).is_some_and(|s| s.trim() == "document")
}

// ---------------------------------------------------------------- configuration

/// How the background agent is started.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The agent CLI (`claude`; `WORDCRAFT_AGENT_COMMAND` picks another that speaks stream-json).
    pub command: String,
    /// The in-app MCP server.
    pub mcp_url: String,
    pub max_turns: u32,
    pub max_budget_usd: f64,
    /// Where the MCP config, the skill and the mode are kept.
    pub data_dir: Option<PathBuf>,
    /// Run in the document's folder (its CLAUDE.md, memory and settings become the agent's
    /// context) instead of the neutral `ArtCraft/agent-sessions/`.
    pub document_folder: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            // Tests never start a real agent, nor write the user's settings.
            command: if cfg!(test) {
                "wordcraft-test-no-agent".into()
            } else {
                std::env::var("WORDCRAFT_AGENT_COMMAND").unwrap_or_else(|_| "claude".into())
            },
            mcp_url: format!("http://127.0.0.1:{}/mcp", crate::server::DEFAULT_PORT),
            max_turns: 40,
            max_budget_usd: 3.0,
            data_dir: None,
            document_folder: false,
        }
    }
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

pub fn config() -> Config {
    match CONFIG.lock() {
        Ok(g) => g.clone().unwrap_or_default(),
        Err(p) => p.into_inner().clone().unwrap_or_default(),
    }
}

pub fn set_config(c: Config) {
    match CONFIG.lock() {
        Ok(mut g) => *g = Some(c),
        Err(p) => *p.into_inner() = Some(c),
    }
}

/// The agent skill, given to the background agent as its system prompt.
pub const SKILL: &str = include_str!("../../../agent-layer/skill/wordcraft-agent/SKILL.md");

// ---------------------------------------------------------------- the document's agent

/// What the document's background agent is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub running: bool,
    pub session_id: String,
    /// The folder it runs in (its project in Claude Desktop).
    pub cwd: String,
    /// Messages sent and not yet finished.
    pub in_flight: usize,
    pub turns: usize,
    pub cost_usd: f64,
    pub last_error: String,
    /// The last few things it said or did, newest last.
    pub recent: Vec<String>,
}

struct Agent {
    child: Child,
    stdin: ChildStdin,
    shared: Arc<Mutex<Status>>,
    /// Started with `--resume`: a quick failure means the session isn't on this machine.
    resumed: bool,
}

/// The session's agent (`Session::ext`).
#[derive(Default)]
struct Slot {
    doc_id: u64,
    agent: Option<Agent>,
    /// The status of an agent that stopped, so its last error stays visible.
    ended: Option<Status>,
    /// The saved session couldn't be resumed: the next start is fresh.
    fresh: bool,
    /// Question ids sent (or given up on after a failed start): each goes once.
    sent: Vec<String>,
}

fn lock(s: &Mutex<Status>) -> MutexGuard<'_, Status> {
    match s.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn slot(s: &mut Session) -> Option<&mut Slot> {
    s.ext.get_mut::<Slot>()
}

/// The background agent's status in this session.
pub fn status(s: &Session) -> Option<Status> {
    let sl = s.ext.get::<Slot>()?;
    sl.agent.as_ref().map(|a| lock(&a.shared).clone()).or_else(|| sl.ended.clone())
}

/// Whether the background agent runs now.
pub fn running(s: &Session) -> bool {
    s.ext.get::<Slot>().is_some_and(|sl| sl.agent.is_some())
}

/// The arguments the agent CLI is started with.
pub fn args(c: &Config, session: &str, resume: bool, mcp_config: &str, skill: Option<&str>, name: &str) -> Vec<String> {
    let mut a: Vec<String> =
        ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"].iter().map(|s| s.to_string()).collect();
    a.extend([if resume { "--resume" } else { "--session-id" }.to_string(), session.to_string()]);
    a.extend(["--name".to_string(), name.to_string()]);
    a.extend(["--mcp-config".to_string(), mcp_config.to_string(), "--strict-mcp-config".to_string()]);
    a.extend(["--allowedTools".to_string(), "mcp__wordcraft__*".to_string()]);
    if let Some(s) = skill {
        a.extend(["--append-system-prompt-file".to_string(), s.to_string()]);
    }
    a.extend(["--max-turns".to_string(), c.max_turns.to_string(), "--max-budget-usd".to_string(), format!("{:.2}", c.max_budget_usd)]);
    a
}

fn user_message(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null}).to_string()
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// Reads the agent's stream-json: turns finished, cost, errors, what it said.
fn read_stdout(out: std::process::ChildStdout, shared: Arc<Mutex<Status>>) {
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Json>(&line) else { continue };
        let mut st = lock(&shared);
        match v.get("type").and_then(Json::as_str) {
            Some("system") if v.get("subtype").and_then(Json::as_str) == Some("init") => {
                if let Some(id) = v.get("session_id").and_then(Json::as_str) {
                    st.session_id = id.to_string();
                }
            }
            Some("assistant") => {
                for part in v.pointer("/message/content").and_then(Json::as_array).cloned().unwrap_or_default() {
                    let line = match part.get("type").and_then(Json::as_str) {
                        Some("text") => part.get("text").and_then(Json::as_str).map(|t| short(t.trim(), 160)),
                        Some("tool_use") => {
                            part.get("name").and_then(Json::as_str).map(|n| format!("→ {}", n.trim_start_matches("mcp__wordcraft__")))
                        }
                        _ => None,
                    };
                    if let Some(l) = line.filter(|l| !l.is_empty()) {
                        st.recent.push(l);
                    }
                }
            }
            Some("result") => {
                st.in_flight = st.in_flight.saturating_sub(1);
                st.turns += 1;
                if let Some(c) = v.get("total_cost_usd").and_then(Json::as_f64) {
                    st.cost_usd = c;
                }
                if v.get("is_error").and_then(Json::as_bool) == Some(true) {
                    let sub = v.get("subtype").and_then(Json::as_str).unwrap_or("error");
                    let text = v.get("result").and_then(Json::as_str).unwrap_or("");
                    st.last_error = short(&format!("{sub}: {text}"), 300);
                }
            }
            _ => {}
        }
        let n = st.recent.len();
        if n > 12 {
            st.recent.drain(..n - 12);
        }
    }
    lock(&shared).running = false;
}

fn read_stderr(err: std::process::ChildStderr, shared: Arc<Mutex<Status>>) {
    for line in BufReader::new(err).lines().map_while(Result::ok) {
        let l = line.trim();
        if !l.is_empty() {
            lock(&shared).last_error = short(l, 300);
        }
    }
}

/// A summary of the document's history, to start a fresh session from.
fn seed(l: &Layer) -> String {
    let mut s = String::from("You are continuing work on this document in a fresh session. What happened so far, from the document's history:\n");
    for e in l.log.iter().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
        let who = if e.actor == Actor::Agent { "agent" } else { "reader" };
        s.push_str(&format!("- {who} {}: {}\n", e.kind, short(&e.text, 200)));
    }
    let open = l.questions.iter().filter(|q| q.status != QuestionStatus::Answered).count();
    s.push_str(&format!("{open} question(s) are open. Call get_context before anything else.\n"));
    s
}

fn write_support_files(c: &Config) -> Result<(String, Option<String>), String> {
    let dir = c.data_dir.clone().ok_or("no folder for the agent's files")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mcp = dir.join("mcp.json");
    let conf = json!({"mcpServers": {"wordcraft": {"type": "http", "url": c.mcp_url}}});
    std::fs::write(&mcp, conf.to_string()).map_err(|e| format!("{}: {e}", mcp.display()))?;
    let skill = dir.join("wordcraft-agent-skill.md");
    let skill = std::fs::write(&skill, SKILL).ok().map(|()| skill.display().to_string());
    Ok((mcp.display().to_string(), skill))
}

/// A random version-4 UUID (a session id), from the system's randomness and the clock.
fn new_uuid() -> String {
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).is_err() {
        let t = crate::now_ms().to_le_bytes();
        let p = std::process::id().to_le_bytes();
        for (i, x) in b.iter_mut().enumerate() {
            *x = t.get(i % 8).copied().unwrap_or(0) ^ p.get(i % 4).copied().unwrap_or(0).rotate_left(i as u32);
        }
    }
    if let Some(x) = b.get_mut(6) {
        *x = (*x & 0x0f) | 0x40;
    }
    if let Some(x) = b.get_mut(8) {
        *x = (*x & 0x3f) | 0x80;
    }
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h.get(0..8).unwrap_or(""),
        h.get(8..12).unwrap_or(""),
        h.get(12..16).unwrap_or(""),
        h.get(16..20).unwrap_or(""),
        h.get(20..32).unwrap_or("")
    )
}

fn display_name(s: &Session) -> String {
    crate::document_title(s)
}

/// Starts this document's background agent (resuming its session when it has one).
pub fn start(s: &mut Session) -> Result<Status, String> {
    crate::ensure(s);
    sync_doc(s);
    if let Some(st) = slot(s).and_then(|sl| sl.agent.as_ref().map(|a| lock(&a.shared).clone())) {
        return Ok(st);
    }
    let c = config();
    let (mcp, skill) = write_support_files(&c)?;
    let l = crate::layer(s);
    let fresh = slot(s).is_some_and(|sl| sl.fresh);
    let (session, resume) = if l.agent_session.is_empty() || fresh { (new_uuid(), false) } else { (l.agent_session.clone(), true) };
    // A neutral, empty folder by default; the document's folder only when asked for.
    let doc_folder = || s.path.as_deref().and_then(|p| std::path::absolute(p).ok()).and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let cwd =
        if c.document_folder { doc_folder() } else { None }.or_else(neutral_folder).or_else(|| c.data_dir.clone()).unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&cwd);
    let name = format!("WordCraft · {}", display_name(s));
    let mut child = Command::new(&c.command)
        .args(args(&c, &session, resume, &mcp, skill.as_deref(), &name))
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't start `{}`: {e}", c.command))?;
    let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        return Err("the agent's pipes didn't open".into());
    };
    let shared = Arc::new(Mutex::new(Status { running: true, session_id: session.clone(), cwd: cwd.display().to_string(), ..Default::default() }));
    {
        let sh = shared.clone();
        let _ = std::thread::Builder::new().name("agent-stdout".into()).spawn(move || read_stdout(stdout, sh));
        let sh = shared.clone();
        let _ = std::thread::Builder::new().name("agent-stderr".into()).spawn(move || read_stderr(stderr, sh));
    }
    let mut agent = Agent { child, stdin, shared, resumed: resume };
    // A fresh session gets the history.
    if !resume && (fresh || !l.log.is_empty()) && writeln!(agent.stdin, "{}", user_message(&seed(&l))).and_then(|()| agent.stdin.flush()).is_ok() {
        lock(&agent.shared).in_flight += 1;
    }
    let st = lock(&agent.shared).clone();
    if let Some(sl) = slot(s) {
        sl.fresh = false;
        sl.ended = None;
        sl.agent = Some(agent);
    }
    if l.agent_session != session {
        let dirty = s.dirty;
        crate::update(s, |l| l.agent_session = session.clone());
        s.dirty = dirty;
    }
    Ok(st)
}

/// Stops this session's agent: end of input, then SIGINT, then (after a grace period) kill.
pub fn stop(s: &mut Session) {
    let Some(sl) = slot(s) else { return };
    let Some(agent) = sl.agent.take() else { return };
    let st = halt(agent);
    sl.ended = Some(st);
}

fn halt(agent: Agent) -> Status {
    let Agent { mut child, stdin, shared, .. } = agent;
    drop(stdin);
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
    }
    let mut done = false;
    for _ in 0..30 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !done {
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut st = lock(&shared).clone();
    st.running = false;
    st.in_flight = 0;
    st
}

/// A new document in the session: the old one's agent stops.
fn sync_doc(s: &mut Session) {
    let id = s.document_id();
    let Some(sl) = slot(s) else { return };
    if sl.doc_id != id {
        if let Some(a) = sl.agent.take() {
            let _ = halt(a);
        }
        *sl = Slot { doc_id: id, ..Default::default() };
    }
}

/// A fresh session for this document: the old one stays in the agent's history; the new one
/// starts from a summary of the document's log.
pub fn reset(s: &mut Session) -> Result<Status, String> {
    stop(s);
    if let Some(sl) = slot(s) {
        sl.fresh = true;
    }
    start(s)
}

/// The message a question is sent as.
pub fn question_text(s: &Session, q: &Json) -> String {
    let get = |k: &str| q.get(k).and_then(Json::as_str).unwrap_or("").to_string();
    if get("kind") == "ping" {
        return format!(
            "Connection check from WordCraft (question id {}): answer it at once with answer_question, text \"here\". Don't investigate.",
            get("id")
        );
    }
    let para = q.get("paragraph").and_then(Json::as_u64).map(|n| format!("paragraph {n}")).unwrap_or_else(|| "the document".into());
    let quote = get("quote");
    let on = if quote.is_empty() { para } else { format!("{para} (“{quote}”)") };
    format!(
        "A question was asked in the document “{}”, on {on}:\n\n“{}”\n\nQuestion id: {}. Claim it (claim_question), investigate with the wordcraft tools, and answer with answer_question, linking the paragraphs and sources you used.",
        display_name(s),
        get("text"),
        get("id")
    )
}

/// Called every frame: in the background mode, starts this document's agent when a question waits
/// and sends each new question once, in order. Leaving the mode stops the agent.
pub fn tick(s: &mut Session) {
    crate::ensure(s);
    sync_doc(s);
    if mode() != Mode::Background {
        if running(s) {
            stop(s);
        }
        return;
    }
    // A finished process: keep its status; a resume that failed starts fresh next time.
    if let Some(sl) = slot(s)
        && let Some(a) = sl.agent.as_mut()
        && !matches!(a.child.try_wait(), Ok(None))
        && let Some(mut a) = sl.agent.take()
    {
        let _ = a.child.wait();
        let mut st = lock(&a.shared).clone();
        st.running = false;
        st.in_flight = 0;
        if a.resumed && st.turns <= 1 {
            sl.fresh = true;
            if st.last_error.is_empty() {
                st.last_error = "the saved session couldn't be resumed here; the next question starts a fresh one".into();
            }
        }
        sl.ended = Some(st);
    }
    let list = match s.run("agent.questions", &json!({"status": "pending", "actor": "agent", "masked": true, "poll": true})) {
        Ok(Json::Array(v)) => v,
        _ => return,
    };
    let sent = slot(s).map(|sl| sl.sent.clone()).unwrap_or_default();
    let new: Vec<Json> = list.into_iter().filter(|q| q.get("id").and_then(Json::as_str).is_some_and(|id| !sent.iter().any(|x| x == id))).collect();
    if new.is_empty() {
        return;
    }
    if !running(s)
        && let Err(e) = start(s)
    {
        // Once per question: no restart loop.
        if let Some(sl) = slot(s) {
            sl.ended = Some(Status { last_error: e, ..Default::default() });
            sl.sent.extend(new.iter().filter_map(|q| q.get("id").and_then(Json::as_str).map(str::to_string)));
        }
        return;
    }
    let texts: Vec<(String, String)> =
        new.iter().filter_map(|q| q.get("id").and_then(Json::as_str).map(|id| (id.to_string(), question_text(s, q)))).collect();
    let Some(sl) = slot(s) else { return };
    let Some(a) = sl.agent.as_mut() else { return };
    for (id, text) in texts {
        if writeln!(a.stdin, "{}", user_message(&text)).and_then(|()| a.stdin.flush()).is_ok() {
            let mut st = lock(&a.shared);
            st.in_flight += 1;
            st.recent.push(format!("← question {id}"));
        } else {
            lock(&a.shared).last_error = "the agent stopped reading its input".into();
        }
        sl.sent.push(id);
    }
}

/// `claude --resume <id>` in the agent's folder: continue its conversation by hand (stop the
/// background agent first: one writer per session).
pub fn resume_command(st: &Status) -> String {
    let c = config();
    let cd = if st.cwd.is_empty() { String::new() } else { format!("cd '{}' && ", st.cwd.replace('\'', "'\\''")) };
    format!("{cd}{} --resume {}", c.command, st.session_id)
}

// ---------------------------------------------------------------- commands (the person's)

pub fn specs() -> Vec<CommandSpec> {
    let loc = "Review › Agent";
    vec![
        CommandSpec::new("agent.agentMode", "Agent Connection Mode", loc, mode_cmd)
            .params(r#"{"mode"?: "connect"|"channel"|"background"|"off"} → the mode and the background agent's status"#)
            .pure(),
        CommandSpec::new("agent.agentReset", "New Agent Session", loc, reset_cmd)
            .params("{} — a fresh background session for this document, from a summary of its history")
            .pure(),
        CommandSpec::new("agent.agentStop", "Stop Background Agent", loc, stop_cmd).params("{}").pure(),
        CommandSpec::new("agent.agentStatus", "Background Agent Status", loc, status_cmd)
            .params("{} → mode, running, session, folder, cost, last error")
            .pure(),
    ]
}

fn refuse_agent(v: &Json) -> Result<(), CmdError> {
    if p::str(v, "actor") == Some("agent") || crate::commands::in_agent_exec() {
        return Err(CmdError::Params("only a person decides how the agent runs".into()));
    }
    Ok(())
}

pub fn status_json(s: &Session) -> Json {
    let st = status(s).unwrap_or_else(|| Status { session_id: crate::layer(s).agent_session.clone(), ..Default::default() });
    json!({
        "mode": mode().id(),
        "command": config().command,
        "running": st.running,
        "sessionId": st.session_id,
        "cwd": st.cwd,
        "inFlight": st.in_flight,
        "turns": st.turns,
        "costUsd": st.cost_usd,
        "lastError": st.last_error,
        "recent": st.recent,
        "resumeCommand": if st.session_id.is_empty() { Json::Null } else { json!(resume_command(&st)) },
    })
}

fn mode_cmd(s: &mut Session, v: &Json) -> CmdResult {
    if let Some(m) = p::str(v, "mode") {
        refuse_agent(v)?;
        set_mode(Mode::parse(m).ok_or_else(|| CmdError::Params("`mode` is connect, channel, background or off".into()))?);
        if mode() != Mode::Background {
            stop(s);
        }
    }
    Ok(status_json(s))
}

fn reset_cmd(s: &mut Session, v: &Json) -> CmdResult {
    refuse_agent(v)?;
    reset(s).map_err(CmdError::Failed)?;
    Ok(status_json(s))
}

fn stop_cmd(s: &mut Session, v: &Json) -> CmdResult {
    refuse_agent(v)?;
    stop(s);
    Ok(status_json(s))
}

fn status_cmd(s: &mut Session, _v: &Json) -> CmdResult {
    Ok(status_json(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_is_started_with_only_wordcraft_and_never_bare() {
        let c = Config { mcp_url: "http://127.0.0.1:7982/mcp".into(), ..Default::default() };
        let a = args(&c, "0b6e…", false, "/tmp/mcp.json", Some("/tmp/skill.md"), "WordCraft · letter.docx");
        let joined = a.join(" ");
        for want in [
            "-p",
            "--input-format stream-json",
            "--output-format stream-json",
            "--verbose",
            "--session-id 0b6e…",
            "--strict-mcp-config",
            "--allowedTools mcp__wordcraft__*",
            "--append-system-prompt-file /tmp/skill.md",
            "--name WordCraft · letter.docx",
            "--max-turns 40",
            "--max-budget-usd 3.00",
        ] {
            assert!(joined.contains(want), "{want} in {joined}");
        }
        assert!(!joined.contains("--bare"));
        assert!(args(&c, "x", true, "m", None, "n").join(" ").contains("--resume x"));
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(u.get(14..15), Some("4"));
        assert!(SKILL.contains("kind ping"), "the skill tells the agent about connection checks");
    }

    #[test]
    fn a_question_reads_as_a_task_and_a_check_as_a_check() {
        crate::install();
        let s = Session::new(crate::sample::engagement_letter());
        let t = question_text(&s, &json!({"id": "q3", "paragraph": 13, "quote": "payable within 30 days", "text": "Is that usual?"}));
        assert!(t.contains("paragraph 13 (“payable within 30 days”)") && t.contains("q3") && t.contains("answer_question"));
        let p = question_text(&s, &json!({"id": "ping1", "kind": "ping"}));
        assert!(p.contains("\"here\"") && p.contains("ping1"));
    }

    #[test]
    fn an_agent_that_cant_start_is_reported_once_and_tests_start_none() {
        // The default command in tests doesn't exist: start fails with a clear error.
        assert_eq!(config().command, "wordcraft-test-no-agent");
        crate::install();
        let mut s = Session::new(crate::sample::engagement_letter());
        let c = Config { data_dir: Some(std::env::temp_dir().join(format!("wordcraft-runner-{}", std::process::id()))), ..Default::default() };
        let e = {
            // start() reads the global config: give it a folder only for this call.
            let old = config();
            set_config(c.clone());
            let r = start(&mut s);
            set_config(old);
            r
        };
        assert!(e.unwrap_err().contains("wordcraft-test-no-agent"));
        assert!(!running(&s));
        let _ = std::fs::remove_dir_all(c.data_dir.unwrap_or_default());
    }
}
