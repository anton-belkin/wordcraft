//! The AItoCraft registry (`~/aitocraft/PROTOCOL.md` §1): this app instance and its open document,
//! in `ArtCraft/agents/wordcraft-<pid>.json`, so the hub (one MCP server configured once in the
//! user's client) can find and reach every open document of every Craft app.
//!
//! Written atomically (a temporary file in the same folder, then a rename) at start, when the
//! document or what is listed about it changes (at most once a second), and every 10 s as a
//! heartbeat; deleted on exit. It holds titles and paths only: no content, no client data.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value as Json, json};

pub const PROTOCOL: &str = "aitocraft/1";
const HEARTBEAT_MS: u64 = 10_000;
const DEBOUNCE_MS: u64 = 1_000;

/// The registry folder of this system.
pub fn folder() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/ArtCraft/agents"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("ArtCraft").join("agents"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share"))).map(|d| d.join("ArtCraft/agents"))
    }
}

struct Inner {
    file: Option<PathBuf>,
    mcp: String,
    /// The documents as the UI last saw them (without their listeners, added at write time).
    documents: Json,
    last_write: u64,
}

static INNER: Mutex<Option<Inner>> = Mutex::new(None);
static DIRTY: AtomicBool = AtomicBool::new(false);

fn inner() -> std::sync::MutexGuard<'static, Option<Inner>> {
    match INNER.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// The registry entry for `documents` (each with its live listeners).
pub fn entry(mcp: &str, documents: &Json, heartbeat: u64) -> Json {
    let docs: Vec<Json> = documents
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| {
                    let mut d = d.clone();
                    let id = d.get("id").and_then(Json::as_str).unwrap_or("").to_string();
                    let listeners: Vec<Json> = crate::presence::listeners_of(&id)
                        .into_iter()
                        .map(|l| json!({"listener": l.listener, "label": l.label, "lastSeen": l.last_seen}))
                        .collect();
                    d["listeners"] = Json::Array(listeners);
                    d
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "protocol": PROTOCOL,
        "app": crate::APP,
        "appTitle": crate::APP_TITLE,
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "mcp": mcp,
        "heartbeat": heartbeat,
        "documents": docs,
    })
}

/// Writes `v` to `path` atomically.
pub fn write_atomic(path: &std::path::Path, v: &Json) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, v.to_string())?;
    std::fs::rename(&tmp, path)
}

fn flush(force: bool) {
    let now = crate::now_ms();
    let mut g = inner();
    let Some(i) = g.as_mut() else { return };
    let due = force
        || now.saturating_sub(i.last_write) >= HEARTBEAT_MS
        || (DIRTY.load(Ordering::Relaxed) && now.saturating_sub(i.last_write) >= DEBOUNCE_MS);
    if !due {
        return;
    }
    let Some(file) = i.file.clone() else { return };
    DIRTY.store(false, Ordering::Relaxed);
    i.last_write = now;
    let v = entry(&i.mcp, &i.documents, now);
    if let Err(e) = write_atomic(&file, &v) {
        log::warn!("AItoCraft registry {}: {e}", file.display());
    }
}

/// Announces this app (its agent server at `mcp`) in `folder` (the system's when `None`), and
/// keeps the file fresh from a background thread.
pub fn start(mcp: String, folder_override: Option<PathBuf>) {
    let file = folder_override.or_else(folder).map(|d| d.join(format!("{}-{}.json", crate::APP, std::process::id())));
    *inner() = Some(Inner { file, mcp, documents: json!([]), last_write: 0 });
    flush(true);
    let _ = std::thread::Builder::new().name("aitocraft-registry".into()).spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if inner().is_none() {
                return;
            }
            flush(false);
        }
    });
}

/// What is open now (from the UI, every frame; cheap when nothing changed). Listeners and the
/// time the window was last focused don't count as a change: the heartbeat carries them.
pub fn observe(documents: Json) {
    let strip = |v: &Json| {
        let mut v = v.clone();
        for d in v.as_array_mut().into_iter().flatten() {
            if let Some(o) = d.as_object_mut() {
                o.remove("listeners");
                o.remove("focusedAt");
            }
        }
        v
    };
    let mut g = inner();
    let Some(i) = g.as_mut() else { return };
    let changed = strip(&i.documents) != strip(&documents);
    i.documents = documents;
    if changed {
        DIRTY.store(true, Ordering::Relaxed);
    }
}

/// Removes this app's file (on exit).
pub fn exit() {
    if let Some(i) = inner().take()
        && let Some(f) = i.file
    {
        let _ = std::fs::remove_file(f);
    }
}

/// This app's registry file, if announced.
pub fn file() -> Option<PathBuf> {
    inner().as_ref().and_then(|i| i.file.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_file_follows_the_protocol_and_goes_away_on_exit() {
        let dir = std::env::temp_dir().join(format!("wordcraft-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        start("http://127.0.0.1:7982/mcp".into(), Some(dir.clone()));
        let f = file().unwrap();
        assert_eq!(f.file_name().unwrap().to_string_lossy(), format!("wordcraft-{}.json", std::process::id()));
        let doc = format!("wordcraft:{}:1", std::process::id());
        let l = crate::presence::attach(&doc, "Cowork · Acme");
        observe(json!([{"id": doc, "title": "contract.docx", "path": "/x/contract.docx", "frontmost": true, "openQuestions": 2}]));
        flush(true);
        let v: Json = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["protocol"], "aitocraft/1");
        assert_eq!(v["app"], "wordcraft");
        assert_eq!(v["appTitle"], "WordCraft");
        assert_eq!(v["mcp"], "http://127.0.0.1:7982/mcp");
        assert_eq!(v["pid"], std::process::id());
        assert!(v["heartbeat"].as_u64().unwrap() > 0);
        assert_eq!(v["documents"][0]["title"], "contract.docx");
        assert_eq!(v["documents"][0]["openQuestions"], 2);
        assert_eq!(v["documents"][0]["listeners"][0]["label"], "Cowork · Acme");
        assert_eq!(v["documents"][0]["listeners"][0]["listener"], l.listener);
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        exit();
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
