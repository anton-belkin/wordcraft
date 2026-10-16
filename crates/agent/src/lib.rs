//! The agent layer (a WordCraft fork feature): a layer of interaction with the user's own agent
//! that lives in the document, not in a chat.
//!
//! - Questions asked on a text range go into a queue in the document; the agent picks them up
//!   through the in-app MCP server (watching `/events`), answers, and the text shows "answer ready".
//! - Notes the agent leaves on ranges: explanations, prepared answers to likely questions,
//!   assumptions and sources, with links (URL + summary) and an Ask button.
//! - A privacy gate: ranges marked as client data read as `{{client.name}}`-style placeholders in
//!   everything the agent gets, and the agent can't write into them.
//! - Templates: placeholders with loops and conditions, rendered with a client from a local CRM
//!   fixture; the agent keeps editing the template and only ever sees placeholders.
//! - A decision history and audit trail of who (person or agent) changed what, on what basis.
//!
//! Everything is stored in the `.docx` as a custom XML part, so it travels with the file.
//!
//! Wiring: [`install`] registers the `agent.*` commands and an observer with the engine
//! (`wordcraft_engine::ext`); the app starts the MCP server with [`server::start`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
pub mod aitocraft;
pub mod anchor;
#[cfg(not(target_arch = "wasm32"))]
pub mod channel;
pub mod commands;
#[cfg(not(target_arch = "wasm32"))]
pub mod cowork;
pub mod crm;
pub mod layer;
pub mod mask;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
pub mod presence;
#[cfg(not(target_arch = "wasm32"))]
pub mod registry;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
pub mod sample;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
#[cfg(not(target_arch = "wasm32"))]
pub mod stdio;
pub mod template;

use std::sync::Arc;

use serde_json::Value as Json;
use wordcraft_doc::Document;
use wordcraft_engine::ext::Observer;
use wordcraft_engine::{CmdResult, Selection, Session};

pub use layer::{Actor, Layer};

/// Milliseconds since the Unix epoch (0 where the platform has no clock).
pub fn now_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Registers the agent commands and the observer that keeps the layer in step with the
/// document. Call before making a [`Session`] (its registry is built then). Safe to call twice.
pub fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        wordcraft_engine::ext::register_commands(commands::specs());
        #[cfg(not(target_arch = "wasm32"))]
        wordcraft_engine::ext::register_commands(runner::specs());
        wordcraft_engine::ext::register_observer(&KEEPER);
    });
}

static LAST_AGENT_CALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// When an agent last called the layer (ms since the epoch; 0 for never), through MCP over HTTP
/// or stdio alike.
pub fn last_agent_call_ms() -> u64 {
    LAST_AGENT_CALL.load(std::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------- the document, as AItoCraft names it

/// The app's name in the AItoCraft registry and in document ids.
pub const APP: &str = "wordcraft";
pub const APP_TITLE: &str = "WordCraft";

fn pid() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::id()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// The open document's id: `wordcraft:<pid>:<doc uid>`, unique across apps and stable while it
/// stays open (a WordCraft window holds one document; opening another gives a new id).
pub fn document_id(s: &Session) -> String {
    format!("{APP}:{}:{}", pid(), s.document_id())
}

/// The document's title: its file name, else its title, else "Untitled".
pub fn document_title(s: &Session) -> String {
    s.file_name
        .clone()
        .or_else(|| s.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| (!s.doc.core.title.is_empty()).then(|| s.doc.core.title.clone()))
        .unwrap_or_else(|| "Untitled".into())
}

static FOCUSED_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The app's window has the focus now.
pub fn note_focus() {
    FOCUSED_AT.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
}

/// The document as the registry and `list_documents` describe it (titles and paths only).
pub fn document_json(s: &Session) -> Json {
    let id = document_id(s);
    let l = layer(s);
    let open = l.questions.iter().filter(|q| q.status != layer::QuestionStatus::Answered).count();
    let listeners: Vec<Json> = presence::listeners_of(&id)
        .into_iter()
        .map(|x| serde_json::json!({"listener": x.listener, "label": x.label, "lastSeen": x.last_seen}))
        .collect();
    let path = s.path.as_ref().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned());
    serde_json::json!({
        "id": id,
        "title": document_title(s),
        "path": path,
        // One document per window: it is the window's.
        "frontmost": true,
        "focusedAt": FOCUSED_AT.load(std::sync::atomic::Ordering::Relaxed),
        "openQuestions": open,
        "listeners": listeners,
    })
}

// ---------------------------------------------------------------- per-session state

/// The preview of a template rendered with a client's data.
#[derive(Clone, Debug)]
pub struct Render {
    /// The CRM id of the client shown.
    pub client: String,
    /// The template, while the rendered document is the one on screen.
    pub template: Document,
    /// Where values were filled in, in the rendered document.
    pub filled: Vec<template::Filled>,
}

/// The layer's state in a session (`Session::ext`).
#[derive(Default)]
pub struct State {
    doc_id: Option<u64>,
    layer: Arc<Layer>,
    pub render: Option<Render>,
    /// The connection check out now (never saved).
    pub ping: Option<presence::Ping>,
}

/// The session's connection check.
pub fn ping(s: &Session) -> Option<presence::Ping> {
    s.ext.get::<State>().and_then(|st| st.ping.clone())
}

/// Loads the layer of the session's document (again after a new document was opened).
pub fn ensure(s: &mut Session) {
    let id = s.document_id();
    let Some(st) = s.ext.get_mut::<State>() else { return };
    if st.doc_id != Some(id) {
        st.doc_id = Some(id);
        st.layer = Arc::new(layer::read_from(&s.doc));
        st.render = None;
    }
}

/// The layer of the session's document.
pub fn layer(s: &Session) -> Arc<Layer> {
    match s.ext.get::<State>() {
        Some(st) if st.doc_id == Some(s.document_id()) => st.layer.clone(),
        _ => Arc::new(layer::read_from(&s.doc)),
    }
}

/// The template preview, if one is shown.
pub fn render_state(s: &Session) -> Option<&Render> {
    s.ext.get::<State>().filter(|st| st.doc_id == Some(s.document_id())).and_then(|st| st.render.as_ref())
}

/// Changes the layer (and marks the document as having unsaved changes).
pub fn update<R>(s: &mut Session, f: impl FnOnce(&mut Layer) -> R) -> R {
    ensure(s);
    let r = match s.ext.get_mut::<State>() {
        Some(st) => {
            let l = Arc::make_mut(&mut st.layer);
            let r = f(l);
            l.revision = l.revision.wrapping_add(1);
            r
        }
        // Never happens (the slot is keyed by type); change a scratch copy rather than panic.
        None => f(&mut Layer::default()),
    };
    s.dirty = true;
    r
}

/// The document the agent works on: the template while a client's rendering is shown.
pub fn agent_doc(s: &Session) -> &Document {
    render_state(s).map_or(&s.doc, |r| &r.template)
}

/// The agent's view of the document (masked).
pub fn view(s: &Session) -> mask::View {
    mask::View::new(agent_doc(s), &layer(s), &crm::secrets())
}

/// Puts `doc` on screen without making it an edit (the selection stays inside it).
pub(crate) fn show_doc(s: &mut Session, doc: Document) {
    let dirty = s.dirty;
    s.doc = doc;
    s.doc.ensure_nonempty();
    let a = s.doc.clamp(&s.sel.anchor);
    let f = s.doc.clamp(&s.sel.focus);
    s.sel = Selection { anchor: a, focus: f };
    s.touch();
    s.dirty = dirty;
}

/// Renders the template for `client` (or, with `None`, puts the template back on screen).
pub fn set_render(s: &mut Session, client: Option<&str>) -> Result<(), String> {
    ensure(s);
    let data = match client {
        Some(id) => Some(crm::client(id).ok_or_else(|| format!("no client `{id}` in the CRM"))?),
        None => None,
    };
    let template = match s.ext.get_mut::<State>().and_then(|st| st.render.take()) {
        Some(r) => r.template,
        None => s.doc.clone(),
    };
    let (Some(id), Some(data)) = (client, data) else {
        show_doc(s, template);
        return Ok(());
    };
    let (mut doc, filled) = template::render(&template, &serde_json::json!({"client": data}));
    // The preview is read only: changes go to the template (the agent's, through the layer).
    doc.settings.protection = Some("readOnly".into());
    show_doc(s, doc);
    if let Some(st) = s.ext.get_mut::<State>() {
        st.render = Some(Render { client: id.to_string(), template, filled });
    }
    Ok(())
}

/// Swaps the template in for a command that must act on it (the agent's edits, Save, Undo) and
/// returns the client whose preview is to be shown again afterwards.
pub(crate) fn template_in(s: &mut Session) -> Option<String> {
    ensure(s);
    let r = s.ext.get_mut::<State>().and_then(|st| st.render.take())?;
    let client = r.client.clone();
    s.doc = r.template;
    Some(client)
}

/// After [`template_in`]: the session's document is the template again; render it for `client`.
pub(crate) fn template_out(s: &mut Session, client: Option<String>) {
    let Some(client) = client else { return };
    // Same document as before the swap (the id bumps only when a new one is opened).
    if let Some(st) = s.ext.get_mut::<State>() {
        st.render = None;
    }
    let _ = set_render(s, Some(&client));
}

// ---------------------------------------------------------------- the observer

struct Keeper;
static KEEPER: Keeper = Keeper;

thread_local! {
    /// Clients whose previews were swapped out by `before` (nested commands stack up).
    static SWAPPED: std::cell::RefCell<Vec<Option<String>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Commands that act on the template while a preview is shown.
fn on_template(id: &str) -> bool {
    matches!(id, "file.save" | "file.saveAs" | "edit.undo" | "edit.redo" | "edit.undoTo")
}

impl Observer for Keeper {
    fn before(&self, s: &mut Session, id: &str, params: &Json) {
        let flag = |k: &str| params.get(k).and_then(Json::as_bool) == Some(true);
        if id.starts_with("agent.") && params.get("actor").and_then(Json::as_str) == Some("agent") {
            if flag("waiting") {
                presence::note_wait();
            } else if !flag("poll") {
                // A real tool call (not a stream polling, not wait_for_questions looping).
                LAST_AGENT_CALL.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
            }
        }
        ensure(s);
        let swapped = if on_template(id) && !commands::in_agent_exec() { template_in(s) } else { None };
        SWAPPED.with(|w| w.borrow_mut().push(swapped));
        // Write the layer into the document just before it is saved.
        if matches!(id, "file.save" | "file.saveAs") {
            let l = layer(s);
            let has_part = layer::read_from(&s.doc) != Layer::default();
            if !l.is_empty() || has_part {
                layer::write_into(&mut s.doc, &l);
            }
        }
    }

    fn after(&self, s: &mut Session, id: &str, params: &Json, result: &CmdResult) {
        let swapped = SWAPPED.with(|w| w.borrow_mut().pop()).flatten();
        let mutated = s.registry.get(id).is_some_and(|c| c.mutates) || matches!(id, "edit.undo" | "edit.redo" | "edit.undoTo");
        // A new document: its own layer (and no preview).
        ensure(s);
        if mutated && result.is_ok() && render_state(s).is_none() {
            refresh_anchors(s);
            commands::log_change(s, id, params);
        }
        if swapped.is_some() {
            template_out(s, swapped);
        }
        // An agent's refused call is the agent's to read, not the reader's status line.
        if result.is_err() && id.starts_with("agent.") && params.get("actor").and_then(Json::as_str) == Some("agent") {
            s.status.clear();
        }
    }
}

/// Finds every anchor again after an edit.
pub(crate) fn refresh_anchors(s: &mut Session) {
    let l = layer(s);
    if l.anchors.is_empty() {
        return;
    }
    let paras = anchor::paragraphs(&s.doc);
    let mut anchors = l.anchors.clone();
    for a in &mut anchors {
        anchor::refresh(&s.doc, &paras, a);
    }
    if anchors != l.anchors
        && let Some(st) = s.ext.get_mut::<State>()
    {
        let l = Arc::make_mut(&mut st.layer);
        l.anchors = anchors;
        l.revision = l.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests;
