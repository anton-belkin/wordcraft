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
pub mod links;
pub mod mask;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
pub mod places;
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
pub mod topic_cmds;
pub mod topics;
pub mod undo;

use std::sync::Arc;

use serde_json::Value as Json;
use wordcraft_doc::{Document, Pos, StoryRef};
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
        wordcraft_engine::ext::register_commands(topic_cmds::specs());
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
    /// Where each block of the rendered document came from in the template.
    pub map: Vec<template::Source>,
}

/// The layer's state in a session (`Session::ext`).
#[derive(Default)]
pub struct State {
    doc_id: Option<u64>,
    layer: Arc<Layer>,
    pub render: Option<Render>,
    /// The connection check out now (never saved).
    pub ping: Option<presence::Ping>,
    /// The person's steps on the layer (undo).
    pub undo: undo::Log,
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
        st.ping = None;
        st.undo = undo::Log::default();
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
    let (mut doc, filled, map) = template::render_mapped(&template, &serde_json::json!({"client": data}));
    // The preview is read only: changes go to the template (the agent's, through the layer).
    doc.settings.protection = Some("readOnly".into());
    show_doc(s, doc);
    if let Some(st) = s.ext.get_mut::<State>() {
        st.render = Some(Render { client: id.to_string(), template, filled, map });
    }
    Ok(())
}

/// A passage of the client's copy on screen, in the template it was rendered from: exact when it
/// comes from template paragraphs shown once; the body of the loop when it is inside a repetition
/// (which one of them is not something a place in the template can say). `true` in the last field
/// for the loop's body.
pub fn template_range(s: &Session, a: &Pos, b: &Pos) -> Option<(Pos, Pos, bool)> {
    let r = render_state(s)?;
    let t = &r.template;
    let (pa, oa, la) = template::to_template(&r.map, &a.path, a.off, template::Bias::Start)?;
    let (pb, ob, lb) = template::to_template(&r.map, &b.path, b.off, template::Bias::End)?;
    // The body of a loop: from the start of its first paragraph to the end of its last.
    let body = |l: &(usize, usize)| -> Option<(Pos, Pos)> {
        let inside: Vec<_> =
            t.para_paths(StoryRef::Body).into_iter().filter(|p| p.0.first().is_some_and(|i| (l.0 + 1..l.1).contains(&(*i as usize)))).collect();
        let (first, last) = (inside.first()?.clone(), inside.last()?.clone());
        let len = anchor::para_text(t, &last).map_or(0, str::len);
        Some((Pos::new(StoryRef::Body, first, 0), Pos::new(StoryRef::Body, last, len)))
    };
    let mut ta = Pos::new(StoryRef::Body, pa, oa);
    let mut tb = Pos::new(StoryRef::Body, pb, ob);
    let common = la.iter().zip(lb).take_while(|(x, y)| x == y).count();
    if let Some(l) = common.checked_sub(1).and_then(|k| la.get(k)) {
        (ta, tb) = body(l)?;
    } else {
        if let Some(l) = la.first() {
            ta = body(l)?.0;
        }
        if let Some(l) = lb.first() {
            tb = body(l)?.1;
        }
    }
    let (ta, tb) = (t.clamp(&ta), t.clamp(&tb));
    (ta <= tb).then_some((ta, tb, !la.is_empty() || !lb.is_empty()))
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
    /// The places before each running command (nested commands stack up).
    static PLACES_BEFORE: std::cell::RefCell<Vec<Option<topics::Before>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// A question asked on the client's copy: the passage in the template (see [`template_range`]).
    static PREVIEW_ASK: std::cell::RefCell<Option<(Pos, Pos, bool)>> = const { std::cell::RefCell::new(None) };
    /// The reader's selection in the client's copy, to put back after a command ran on the template.
    static READER_SEL: std::cell::RefCell<Vec<Option<(Pos, Pos)>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The selection a Copy widened over places' bookmarks, to put back after it.
    static COPY_SEL: std::cell::RefCell<Vec<Option<Selection>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Commands that put back an earlier state of the document.
fn is_history(id: &str) -> bool {
    matches!(id, "edit.undo" | "edit.redo" | "edit.undoTo")
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
        // A question asked on the client's copy goes on the template paragraph it was rendered from.
        let mut asked = None;
        if id == "agent.ask" && !commands::is_agent(params) && render_state(s).is_some() && !commands::in_agent_exec() {
            asked = topic_cmds::reader_range(s, params).ok().and_then(|(a, b)| template_range(s, &a, &b));
        }
        let reader_sel = asked.is_some().then(|| {
            let (a, b) = s.sel.ordered();
            (text_offset(&s.doc, &a), text_offset(&s.doc, &b))
        });
        READER_SEL.with(|w| w.borrow_mut().push(reader_sel));
        let swapped = if (on_template(id) || asked.is_some()) && !commands::in_agent_exec() { template_in(s) } else { None };
        if let Some((a, b, _)) = &asked {
            s.sel = Selection { anchor: a.clone(), focus: b.clone() };
        }
        PREVIEW_ASK.with(|w| *w.borrow_mut() = asked);
        SWAPPED.with(|w| w.borrow_mut().push(swapped));
        let mutates = s.registry.get(id).is_some_and(|c| c.mutates) || is_history(id);
        let places = (mutates && render_state(s).is_none()).then(|| topics::before(s));
        PLACES_BEFORE.with(|w| w.borrow_mut().push(places));
        // Copy and Cut of a whole place take its bookmark along.
        let widened = if matches!(id, "edit.copy" | "edit.cut") && render_state(s).is_none() { topics::widen_for_clipboard(s) } else { None };
        COPY_SEL.with(|w| w.borrow_mut().push(widened));
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
        let places = PLACES_BEFORE.with(|w| w.borrow_mut().pop()).flatten();
        if let Some(sel) = COPY_SEL.with(|w| w.borrow_mut().pop()).flatten()
            && id == "edit.copy"
        {
            s.sel = sel;
        }
        let mutated = s.registry.get(id).is_some_and(|c| c.mutates) || is_history(id);
        // A new document: its own layer (and no preview).
        ensure(s);
        if mutated && result.is_ok() && render_state(s).is_none() {
            if is_history(id) {
                // The person's steps on the layer follow the document back and forth.
                undo::sync(s);
            }
            if let Some(before) = &places {
                topics::reconcile(s, is_history(id), before);
            }
            refresh_anchors(s);
            commands::log_change(s, id, params);
        }
        PREVIEW_ASK.with(|w| w.borrow_mut().take());
        if swapped.is_some() {
            template_out(s, swapped);
        }
        if let Some((a, b)) = READER_SEL.with(|w| w.borrow_mut().pop()).flatten() {
            // The re-rendered copy has the new place's markers: the same text stays selected.
            let a = s.doc.clamp(&from_text_offset(&s.doc, &a, true));
            let b = s.doc.clamp(&from_text_offset(&s.doc, &b, false));
            s.sel = Selection { anchor: a, focus: b };
        }
        // An agent's refused call is the agent's to read, not the reader's status line.
        if result.is_err() && id.starts_with("agent.") && params.get("actor").and_then(Json::as_str) == Some("agent") {
            s.status.clear();
        }
    }
}

/// A position with its offset counted in the paragraph's text without inline markers.
fn text_offset(doc: &Document, p: &Pos) -> Pos {
    let text = anchor::para_text(doc, &p.path).unwrap_or("");
    let objs = text.get(..p.off.min(text.len())).map_or(0, |t| t.matches(wordcraft_doc::para::OBJ).count());
    Pos { off: p.off.saturating_sub(objs * wordcraft_doc::para::OBJ.len_utf8()), ..p.clone() }
}

/// Back from [`text_offset`]: past the markers there (`after`, for a start) or before them.
fn from_text_offset(doc: &Document, p: &Pos, after: bool) -> Pos {
    let text = anchor::para_text(doc, &p.path).unwrap_or("");
    let mut plain = 0usize;
    let mut off = text.len();
    for (i, c) in text.char_indices() {
        if c == wordcraft_doc::para::OBJ {
            continue;
        }
        if plain == p.off {
            off = i;
            break;
        }
        plain += c.len_utf8();
    }
    if !after {
        // Before the markers just ahead of the text that follows.
        while off > 0 && text.get(..off).is_some_and(|t| t.ends_with(wordcraft_doc::para::OBJ)) {
            off -= wordcraft_doc::para::OBJ.len_utf8();
        }
    }
    Pos { off, ..p.clone() }
}

/// The passage of a question being asked on the client's copy, in the template (once).
pub(crate) fn take_preview_ask() -> Option<(Pos, Pos, bool)> {
    PREVIEW_ASK.with(|w| w.borrow_mut().take())
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
