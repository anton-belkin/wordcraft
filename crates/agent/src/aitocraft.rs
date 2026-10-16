//! AItoCraft, the one MCP hub for every Craft app (`~/aitocraft/PROTOCOL.md` §3): where its binary
//! is, installing it in Claude Desktop (`aitocraft --install-desktop`), and what to tell Claude.

use std::path::PathBuf;
use std::sync::Mutex;

/// The phrase that starts the work in a conversation ("Copy phrase for Claude").
pub fn phrase(title: &str) -> String {
    format!(
        "Work with me on \"{title}\" in {} through AItoCraft: attach to it and answer my questions from the document as they come.",
        crate::APP_TITLE
    )
}

/// The hub's binary: on PATH, else where it is usually built or installed.
pub fn binary() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let mut candidates: Vec<PathBuf> = path_dirs.into_iter().map(|d| d.join("aitocraft")).collect();
    if let Some(h) = &home {
        candidates.push(h.join(".local/bin/aitocraft"));
        candidates.push(h.join(".cargo/bin/aitocraft"));
        candidates.push(h.join("aitocraft/target/release/aitocraft"));
    }
    candidates.extend(["/opt/homebrew/bin/aitocraft", "/usr/local/bin/aitocraft"].map(PathBuf::from));
    candidates.into_iter().find(|p| p.is_file())
}

/// `claude mcp add` for the hub.
pub fn claude_code_command(bin: Option<&std::path::Path>) -> String {
    let b = bin.map_or_else(|| "<path to>/aitocraft".to_string(), |p| p.display().to_string());
    format!("claude mcp add --scope user aitocraft -- '{}'", b.replace('\'', "'\\''"))
}

/// Starting Claude Code with the hub's channel (questions pushed into the session).
pub const CHANNEL_COMMAND: &str = "claude --dangerously-load-development-channels server:aitocraft";

/// Where installing stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Install {
    #[default]
    Idle,
    Running,
    /// What the installer said.
    Done(String),
    Failed(String),
}

static INSTALL: Mutex<Install> = Mutex::new(Install::Idle);

fn set(i: Install) {
    match INSTALL.lock() {
        Ok(mut g) => *g = i,
        Err(p) => *p.into_inner() = i,
    }
}

pub fn install_state() -> Install {
    match INSTALL.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

/// Runs `aitocraft --install-desktop` in the background (it adds the hub to Claude Desktop's settings).
pub fn install_desktop() {
    let Some(bin) = binary() else {
        set(Install::Failed("AItoCraft isn't installed: build it (~/aitocraft) or put `aitocraft` on PATH".into()));
        return;
    };
    if install_state() == Install::Running {
        return;
    }
    set(Install::Running);
    let spawned = std::thread::Builder::new().name("aitocraft-install".into()).spawn(move || {
        let out = std::process::Command::new(&bin).arg("--install-desktop").output();
        set(match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                Install::Done(if text.is_empty() {
                    "AItoCraft is installed in Claude Desktop: quit Claude (Cmd+Q) and open it again".into()
                } else {
                    text
                })
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                Install::Failed(if err.is_empty() { format!("aitocraft --install-desktop failed ({})", o.status) } else { err })
            }
            Err(e) => Install::Failed(format!("can't run {}: {e}", bin.display())),
        });
    });
    if spawned.is_err() {
        set(Install::Failed("couldn't start the installer".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_and_commands_say_what_the_protocol_says() {
        assert_eq!(
            phrase("contract.docx"),
            "Work with me on \"contract.docx\" in WordCraft through AItoCraft: attach to it and answer my questions from the document as they come."
        );
        assert_eq!(claude_code_command(Some(std::path::Path::new("/x/aitocraft"))), "claude mcp add --scope user aitocraft -- '/x/aitocraft'");
        assert!(CHANNEL_COMMAND.ends_with("server:aitocraft"));
    }
}
