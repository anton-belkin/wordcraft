//! What the agent knows: the Agent tab's Context group (the same in GridCraft and DeckCraft).
//!
//! - **No context**: only this document; the app's background client, in its neutral empty folder
//!   (optionally allowed the web).
//! - **Claude Cowork**: the person's Cowork chat, its project and memory (AItoCraft, and the chat's
//!   `wait_for_questions` loop).
//! - **Claude Code**: the person's Claude Code session (AItoCraft's channel), or a project folder
//!   the background client runs in (that folder's CLAUDE.md, memory and files).
//!
//! The context is the document's, saved in its agent layer; a new document starts with the last
//! one used in the app. It decides the connection mode ([`presence::Mode`]) and how the background
//! client is started ([`crate::runner`]). A project folder written in a file is used only once this
//! machine's person has chosen it here: a document someone sent can't point the agent at a folder.
//!
//! Also the "Get ready" checks: is the `claude` CLI logged in, is AItoCraft added to Claude Code
//! or Claude Desktop. They run in the background and are cached.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::presence::{self, Mode};
use crate::undo::{self, Op};

#[cfg(not(target_arch = "wasm32"))]
pub use ready::*;

/// Which context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Only this document, the background client.
    None,
    /// The person's Cowork chat.
    #[default]
    Cowork,
    /// The person's Claude Code session or a project folder.
    Code,
}

/// How Claude Code comes in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeWay {
    #[default]
    Session,
    Folder,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::None, Kind::Cowork, Kind::Code];

    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "no context" | "document" => Some(Kind::None),
            "cowork" | "claude cowork" => Some(Kind::Cowork),
            "code" | "claude code" => Some(Kind::Code),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Kind::None => "none",
            Kind::Cowork => "cowork",
            Kind::Code => "code",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::None => "No context",
            Kind::Cowork => "Claude Cowork",
            Kind::Code => "Claude Code",
        }
    }
    /// The line under the name in the menu.
    pub fn line(self) -> &'static str {
        match self {
            Kind::None => "Only this document. Runs in the background.",
            Kind::Cowork => "Your Cowork chat, its project and memory.",
            Kind::Code => "Your Claude Code session or a project folder.",
        }
    }
    /// What the agent knows (the menu's chips).
    pub fn chips(self) -> &'static [&'static str] {
        match self {
            Kind::None => &["Document"],
            Kind::Cowork => &["Document", "Your chat"],
            Kind::Code => &["Document", "Project folder"],
        }
    }
}

impl CodeWay {
    pub fn parse(s: &str) -> Option<CodeWay> {
        match s.trim().to_ascii_lowercase().as_str() {
            "session" | "my session" => Some(CodeWay::Session),
            "folder" | "project folder" | "project" => Some(CodeWay::Folder),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            CodeWay::Session => "session",
            CodeWay::Folder => "folder",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CodeWay::Session => "My session",
            CodeWay::Folder => "Project folder",
        }
    }
}

/// A document's context (in its agent layer).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub code: CodeWay,
    /// The project folder (Claude Code › Project folder).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// No context: the background client may also search and fetch the web.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web: bool,
}

impl Context {
    /// The connection mode it means.
    pub fn mode(&self) -> Mode {
        match (self.kind, self.code) {
            (Kind::None, _) | (Kind::Code, CodeWay::Folder) => Mode::Background,
            (Kind::Cowork, _) => Mode::Connect,
            (Kind::Code, CodeWay::Session) => Mode::Channel,
        }
    }
    /// The background client answers (no listener needed).
    pub fn background(&self) -> bool {
        self.mode() == Mode::Background
    }
    /// The project folder the background client runs in: chosen on this machine, and there.
    pub fn project_folder(&self) -> Option<PathBuf> {
        (self.kind == Kind::Code && self.code == CodeWay::Folder && !self.folder.is_empty() && approved(&self.folder))
            .then(|| PathBuf::from(&self.folder))
            .filter(|p| p.is_dir())
    }
    /// The web for the background client (No context only).
    pub fn web_on(&self) -> bool {
        self.kind == Kind::None && self.web
    }
    /// "Claude Code · Project folder".
    pub fn describe(&self) -> String {
        match self.kind {
            Kind::Code => format!("{} · {}", self.kind.label(), self.code.label()),
            Kind::None if self.web => format!("{} · web", self.kind.label()),
            k => k.label().to_string(),
        }
    }
    /// The context a connection mode stands for (keeping the rest).
    pub fn with_mode(mut self, m: Mode) -> Context {
        match m {
            Mode::Connect => self.kind = Kind::Cowork,
            Mode::Channel => {
                self.kind = Kind::Code;
                self.code = CodeWay::Session;
            }
            Mode::Background if !(self.kind == Kind::Code && self.code == CodeWay::Folder) => self.kind = Kind::None,
            _ => {}
        }
        self
    }
    pub fn json(&self) -> Json {
        json!({"kind": self.kind.id(), "label": self.describe(), "code": self.code.id(), "folder": self.folder, "web": self.web, "mode": self.mode().id()})
    }
}

// ---------------------------------------------------------------- the app's side

fn data_dir() -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    return crate::runner::config().data_dir;
    #[cfg(target_arch = "wasm32")]
    None
}

/// The mode remembered before the Context group.
fn old_mode() -> Option<Mode> {
    #[cfg(not(target_arch = "wasm32"))]
    return crate::runner::load_mode();
    #[cfg(target_arch = "wasm32")]
    None
}

static LAST: Mutex<Option<Context>> = Mutex::new(None);
static APPROVED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// The context last used in the app (what a new document starts with).
pub fn last() -> Context {
    if let Some(c) = locked(&LAST).clone() {
        return c;
    }
    let dir = data_dir();
    let saved = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("context.json")).ok()).and_then(|t| serde_json::from_str(&t).ok());
    // Before the Context group: the mode file.
    let c = saved.unwrap_or_else(|| old_mode().map_or_else(Context::default, |m| Context::default().with_mode(m)));
    *locked(&LAST) = Some(c.clone());
    c
}

/// Remembers `c` as the last used.
pub fn set_last(c: &Context) {
    *locked(&LAST) = Some(c.clone());
    if let Some(d) = data_dir() {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("context.json"), serde_json::to_string(c).unwrap_or_default());
    }
}

/// Forgets the remembered context (the app read its settings folder anew).
pub fn reload() {
    *locked(&LAST) = None;
    locked(&APPROVED).clear();
}

fn approved_file() -> Option<PathBuf> {
    data_dir().map(|d| d.join("folders"))
}

/// Whether the person chose `folder` on this machine.
pub fn approved(folder: &str) -> bool {
    if locked(&APPROVED).iter().any(|f| f == folder) {
        return true;
    }
    approved_file().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|t| t.lines().any(|l| l == folder))
}

/// The person chose `folder` here.
pub fn approve(folder: &str) {
    if approved(folder) {
        return;
    }
    locked(&APPROVED).push(folder.to_string());
    if let Some(f) = approved_file() {
        let mut t = std::fs::read_to_string(&f).unwrap_or_default();
        t.push_str(folder);
        t.push('\n');
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&f, t);
    }
}

/// The context of the session's document (its own, else the last used in the app).
pub fn of(s: &Session) -> Context {
    crate::layer(s).context.clone().unwrap_or_else(last)
}

/// Makes the connection mode follow the document's context (unless an agent connection was
/// switched off on purpose).
pub fn apply(s: &Session) {
    let want = of(s).mode();
    let now = presence::mode();
    if now != Mode::Off && now != want {
        presence::store_mode(want);
    }
}

// ---------------------------------------------------------------- commands (the person's)

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("agent.setContext", "Agent Context", "Review › Agent", set_context).params(
            r#"{"kind"?: "none"|"cowork"|"code", "code"?: "session"|"folder", "folder"?: string (a folder on this computer), "web"?: bool} — what the agent knows, for this document (an undo step)"#,
        ),
        CommandSpec::new("agent.context", "Agent Context (Read)", "Review › Agent", context_cmd).params("{} → this document's context").pure(),
    ]
}

fn refuse_agent(v: &Json) -> Result<(), CmdError> {
    if p::str(v, "actor") == Some("agent") || crate::commands::in_agent_exec() {
        return Err(CmdError::Params("only a person decides what the agent knows".into()));
    }
    Ok(())
}

fn context_cmd(s: &mut Session, _v: &Json) -> CmdResult {
    let c = of(s);
    let mut j = c.json();
    j["own"] = json!(crate::layer(s).context.is_some());
    j["projectFolder"] = json!(c.project_folder().map(|p| p.display().to_string()));
    Ok(j)
}

/// Sets this document's context (remembered as the app's last used too).
pub fn set_context(s: &mut Session, v: &Json) -> CmdResult {
    refuse_agent(v)?;
    crate::ensure(s);
    // What it was in effect (a document without its own takes the app's last, which changes now).
    let before = Some(of(s));
    let mut c = of(s);
    if let Some(k) = p::str(v, "kind") {
        c.kind = Kind::parse(k).ok_or_else(|| CmdError::Params("`kind` is none, cowork or code".into()))?;
    }
    if let Some(w) = p::str(v, "code") {
        c.code = CodeWay::parse(w).ok_or_else(|| CmdError::Params("`code` is session or folder".into()))?;
    }
    if let Some(f) = p::str(v, "folder") {
        let f = f.trim();
        if f.len() > 4096 {
            return Err(CmdError::Params("`folder` is too long".into()));
        }
        if !f.is_empty() {
            if !std::path::Path::new(f).is_dir() {
                return Err(CmdError::Params(format!("no folder `{f}` on this computer")));
            }
            approve(f);
        }
        c.folder = f.to_string();
    }
    if let Some(b) = p::bool(v, "web") {
        c.web = b;
    }
    set_last(&c);
    presence::store_mode(c.mode());
    let after = Some(c.clone());
    if crate::layer(s).context != after {
        let a = after.clone();
        crate::update(s, |l| l.context = a);
        undo::push(s, Op::Context { before, after });
        let who = s.author.clone();
        let text = format!("Context: {}", c.describe());
        crate::update(s, |l| {
            l.log(crate::layer::LogEntry {
                at: crate::now_ms(),
                actor: crate::layer::Actor::Human,
                kind: "decision".into(),
                text,
                command: "agent.setContext".into(),
                who,
                ..Default::default()
            })
        });
    }
    context_cmd(s, v)
}

// ---------------------------------------------------------------- Get ready checks (native only)

#[cfg(not(target_arch = "wasm32"))]
mod ready {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;

    use serde_json::Value as Json;

    use super::locked;

    /// What a background check found.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Check {
        /// When it last finished (ms; 0: never).
        pub at: u64,
        pub ok: bool,
        /// Why not, in a few words.
        pub why: String,
        running: bool,
    }

    static LOGIN: Mutex<Check> = Mutex::new(Check { at: 0, ok: false, why: String::new(), running: false });
    static CODE_HUB: Mutex<Check> = Mutex::new(Check { at: 0, ok: false, why: String::new(), running: false });

    /// Runs `f` in the background unless a run is out or the last one is younger than `max_age_ms`.
    fn refresh(slot: &'static Mutex<Check>, max_age_ms: u64, f: fn() -> (bool, String)) -> Check {
        let now = crate::now_ms();
        let mut g = locked(slot);
        if !g.running && (g.at == 0 || now.saturating_sub(g.at) >= max_age_ms) && !cfg!(test) {
            g.running = true;
            let spawned = std::thread::Builder::new().name("agent-ready-check".into()).spawn(move || {
                let (ok, why) = f();
                let mut g = locked(slot);
                *g = Check { at: crate::now_ms(), ok, why, running: false };
            });
            if spawned.is_err() {
                g.running = false;
            }
        }
        g.clone()
    }

    fn quiet(cmd: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new(cmd).args(args).stdin(Stdio::null()).output()
    }

    fn check_login() -> (bool, String) {
        let cmd = crate::runner::config().command;
        match quiet(&cmd, &["auth", "status"]) {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                match serde_json::from_str::<Json>(text.trim()).ok().and_then(|j| j.get("loggedIn").and_then(Json::as_bool)) {
                    Some(true) => (true, String::new()),
                    Some(false) => (false, "Claude isn't logged in on this computer".into()),
                    // Another agent CLI: it runs, that's enough.
                    None if o.status.success() => (true, String::new()),
                    None => (false, "Claude isn't logged in on this computer".into()),
                }
            }
            Err(_) => (false, format!("`{cmd}` isn't installed (Claude Code's command line)")),
        }
    }

    fn check_code_hub() -> (bool, String) {
        let cmd = crate::runner::config().command;
        match quiet(&cmd, &["mcp", "get", "aitocraft"]) {
            Ok(o) if o.status.success() => (true, String::new()),
            Ok(_) => (false, "AItoCraft isn't added to Claude Code".into()),
            Err(_) => (false, format!("`{cmd}` isn't installed (Claude Code's command line)")),
        }
    }

    /// Is the `claude` CLI there and logged in (checked every `max_age_ms` at most).
    pub fn login(max_age_ms: u64) -> Check {
        refresh(&LOGIN, max_age_ms, check_login)
    }

    /// Is AItoCraft added to Claude Code (`claude mcp get aitocraft`).
    pub fn code_hub(max_age_ms: u64) -> Check {
        refresh(&CODE_HUB, max_age_ms, check_code_hub)
    }

    /// Is AItoCraft in Claude Desktop's settings (Cowork uses them).
    pub fn desktop_hub() -> bool {
        if matches!(crate::aitocraft::install_state(), crate::aitocraft::Install::Done(_)) {
            return true;
        }
        crate::cowork::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Json>(&t).ok())
            .is_some_and(|j| j.get("mcpServers").and_then(|m| m.get("aitocraft")).is_some())
    }

    /// Opens a terminal running `command` (Get ready › Log in to Claude).
    pub fn open_terminal(command: &str) -> Result<(), String> {
        let r = if cfg!(target_os = "macos") {
            let script =
                format!("tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell", command.replace('\\', "\\\\").replace('"', "\\\""));
            Command::new("osascript").args(["-e", &script]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        } else if cfg!(target_os = "windows") {
            Command::new("cmd").args(["/C", "start", "cmd", "/K", command]).spawn()
        } else {
            Command::new("x-terminal-emulator").args(["-e", "sh", "-c", &format!("{command}; exec sh")]).spawn()
        };
        r.map(|_| ()).map_err(|e| format!("couldn't open a terminal: {e}"))
    }

    /// Get ready › Log in to Claude.
    pub fn log_in() -> Result<(), String> {
        let r = open_terminal(&format!("{} auth login", crate::runner::config().command));
        // Look again soon.
        locked(&LOGIN).at = 1;
        r
    }

    /// Get ready › Install AItoCraft (Claude Code): `claude mcp add --scope user aitocraft -- <hub>`.
    pub fn install_code_hub() -> Result<(), String> {
        let hub = crate::aitocraft::binary().ok_or("AItoCraft isn't installed yet: build it (~/aitocraft) or put `aitocraft` on PATH")?;
        let cmd = crate::runner::config().command;
        let o = quiet(&cmd, &["mcp", "add", "--scope", "user", "aitocraft", "--", &hub.display().to_string()])
            .map_err(|e| format!("can't run `{cmd}`: {e}"))?;
        locked(&CODE_HUB).at = 1;
        if o.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Err(if err.is_empty() { "claude mcp add failed".into() } else { err })
        }
    }

    /// Picks a folder with the system's chooser (blocking: call it off the UI thread).
    pub fn choose_folder() -> Option<String> {
        let out = if cfg!(target_os = "macos") {
            quiet("osascript", &["-e", "POSIX path of (choose folder with prompt \"The project folder the agent works in\")"]).ok()?
        } else if cfg!(target_os = "windows") {
            quiet(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }",
                ],
            )
            .ok()?
        } else {
            quiet("zenity", &["--file-selection", "--directory"]).ok()?
        };
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let p = if p.len() > 1 { p.trim_end_matches('/').to_string() } else { p };
        (out.status.success() && !p.is_empty() && Path::new(&p).is_dir()).then_some(p)
    }
}
