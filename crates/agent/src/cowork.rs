//! Connecting Claude Desktop (and Cowork in it) to the agent layer: the `wordcraft` entry in
//! `claude_desktop_config.json`, pointing at this app's stdio mode (`wordcraft --agent-stdio`).
//!
//! Claude Desktop keeps that file in memory and writes it back when it quits, so an entry added
//! while it runs is lost. [`connect`] therefore writes at once only when Claude isn't running;
//! otherwise it waits (in the background) until Claude has quit, then writes. Other servers and
//! settings are kept as they are, and the old file is copied aside first.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value as Json, json};

/// The server's name in the config.
pub const SERVER_NAME: &str = "wordcraft";
/// The app's flag for the stdio MCP mode.
pub const STDIO_FLAG: &str = "--agent-stdio";
/// Longest wait for Claude to quit.
const WAIT_SECS: u64 = 30 * 60;

/// Where Claude Desktop keeps its config on this system.
pub fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Claude").join("claude_desktop_config.json"))
    } else {
        home.map(|h| h.join(".config/Claude/claude_desktop_config.json"))
    }
}

/// The config entry that starts `exe` in stdio mode, reaching the app's control channel on `control_port`.
pub fn entry(exe: &Path, control_port: u16) -> Json {
    json!({"command": exe.to_string_lossy(), "args": [STDIO_FLAG, "--control", format!("127.0.0.1:{control_port}")]})
}

/// What the config says about us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Configured {
    /// No Claude Desktop config on this machine.
    NoClaude,
    Missing,
    /// There, but pointing somewhere else (another build).
    Different,
    Yes,
}

pub fn configured(path: &Path, want: &Json) -> Configured {
    let Ok(text) = std::fs::read_to_string(path) else {
        return if path.parent().is_some_and(Path::exists) { Configured::Missing } else { Configured::NoClaude };
    };
    let Ok(v) = serde_json::from_str::<Json>(&text) else { return Configured::Missing };
    match v.get("mcpServers").and_then(|s| s.get(SERVER_NAME)) {
        None => Configured::Missing,
        Some(e) if e == want => Configured::Yes,
        Some(_) => Configured::Different,
    }
}

/// Adds (or replaces) our entry, keeping everything else; copies the old file to
/// `<file>.bak-wordcraft-<time>` first. Refuses a file that isn't a JSON object.
pub fn write(path: &Path, want: &Json) -> Result<Option<PathBuf>, String> {
    let (mut v, backup) = match std::fs::read_to_string(path) {
        Ok(text) => {
            let v: Json = serde_json::from_str(&text).map_err(|e| format!("{} isn't valid JSON ({e}); not touching it", path.display()))?;
            if !v.is_object() {
                return Err(format!("{} isn't a JSON object; not touching it", path.display()));
            }
            let mut name = path.as_os_str().to_os_string();
            name.push(format!(".bak-wordcraft-{}", crate::now_ms()));
            let backup = PathBuf::from(name);
            std::fs::write(&backup, text).map_err(|e| format!("backup {}: {e}", backup.display()))?;
            (v, Some(backup))
        }
        Err(_) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            (json!({}), None)
        }
    };
    let Some(o) = v.as_object_mut() else { return Err("not a JSON object".into()) };
    let servers = o.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else { return Err("`mcpServers` isn't an object; not touching it".into()) };
    servers.insert(SERVER_NAME.into(), want.clone());
    let mut text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    text.push('\n');
    // Through a temporary file, so a crash never leaves half a config.
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".wordcraft-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(backup)
}

/// Whether Claude Desktop is running (macOS and Windows; elsewhere it is assumed not to be).
pub fn claude_running() -> bool {
    if cfg!(target_os = "macos") {
        let Ok(out) = std::process::Command::new("/bin/ps").args(["-axo", "comm="]).output() else { return false };
        String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim_end().ends_with("/Claude.app/Contents/MacOS/Claude"))
    } else if cfg!(target_os = "windows") {
        let Ok(out) = std::process::Command::new("tasklist").args(["/FI", "IMAGENAME eq Claude.exe", "/NH"]).output() else { return false };
        String::from_utf8_lossy(&out.stdout).contains("Claude.exe")
    } else {
        false
    }
}

/// Where connecting stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    /// Waiting for Claude Desktop to quit.
    Waiting,
    /// Written; Claude must be (re)opened to load it.
    Done {
        backup: Option<PathBuf>,
    },
    Failed(String),
}

static STATE: Mutex<State> = Mutex::new(State::Idle);

fn set(s: State) {
    match STATE.lock() {
        Ok(mut g) => *g = s,
        Err(p) => *p.into_inner() = s,
    }
}

pub fn state() -> State {
    match STATE.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

/// Adds the entry now if Claude Desktop isn't running, else as soon as it has quit.
pub fn connect(want: Json) {
    let Some(path) = config_path() else {
        set(State::Failed("no place for Claude Desktop's config on this system".into()));
        return;
    };
    if !claude_running() {
        set(match write(&path, &want) {
            Ok(backup) => State::Done { backup },
            Err(e) => State::Failed(e),
        });
        return;
    }
    if state() == State::Waiting {
        return;
    }
    set(State::Waiting);
    let spawned = std::thread::Builder::new().name("cowork-connect".into()).spawn(move || {
        for _ in 0..WAIT_SECS {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if claude_running() {
                continue;
            }
            // Let it finish writing its own copy first.
            std::thread::sleep(std::time::Duration::from_millis(1500));
            if claude_running() {
                continue;
            }
            set(match write(&path, &want) {
                Ok(backup) => State::Done { backup },
                Err(e) => State::Failed(e),
            });
            return;
        }
        set(State::Failed("Claude Desktop didn't quit within 30 minutes; press Connect Cowork again".into()));
    });
    if spawned.is_err() {
        set(State::Failed("couldn't start waiting for Claude to quit".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wordcraft-cowork-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn adds_the_entry_and_keeps_everything_else() {
        let d = dir("keep");
        let p = d.join("claude_desktop_config.json");
        std::fs::write(
            &p,
            r#"{"globalShortcut": "Alt+Space", "mcpServers": {"other": {"command": "x", "env": {"TOKEN": "secret"}}}, "preferences": {"a": 1}}"#,
        )
        .unwrap();
        let want = entry(Path::new("/Applications/WordCraft.app/Contents/MacOS/wordcraft"), 7981);
        assert_eq!(configured(&p, &want), Configured::Missing);
        let backup = write(&p, &want).unwrap().unwrap();
        let v: Json = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["other"]["env"]["TOKEN"], "secret");
        assert_eq!(v["globalShortcut"], "Alt+Space");
        assert_eq!(v["preferences"]["a"], 1);
        assert_eq!(v["mcpServers"]["wordcraft"]["args"], json!(["--agent-stdio", "--control", "127.0.0.1:7981"]));
        assert!(std::fs::read_to_string(backup).unwrap().contains("secret"));
        assert_eq!(configured(&p, &want), Configured::Yes);
        assert_eq!(configured(&p, &entry(Path::new("/elsewhere"), 7981)), Configured::Different);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_broken_config_is_left_alone_and_a_missing_one_is_made() {
        let d = dir("broken");
        let p = d.join("claude_desktop_config.json");
        std::fs::write(&p, "{ not json").unwrap();
        assert!(write(&p, &json!({"command": "x"})).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ not json");
        let q = d.join("new/claude_desktop_config.json");
        assert_eq!(configured(&q, &json!({})), Configured::NoClaude);
        assert_eq!(write(&q, &json!({"command": "x"})).unwrap(), None);
        assert_eq!(configured(&q, &json!({"command": "x"})), Configured::Yes);
        let _ = std::fs::remove_dir_all(&d);
    }
}
