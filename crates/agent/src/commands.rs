//! The `agent.*` commands. The reader's side (asking, marking client data, choosing a client for
//! a template) and the agent's side (answers, notes, masked reads, edits with a reason) are both
//! commands, so the UI, the control channel and the in-app MCP server reach them the same way.
//!
//! The agent's side is masked: it reads paragraphs with client data as `{{label}}` placeholders,
//! points at text by paragraph number and quote (as it read them), and every string it gets back
//! is scrubbed of real client data.

use std::cell::{Cell, RefCell};

use serde_json::{Value as Json, json};
use wordcraft_doc::{Path, StoryRef};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

use crate::anchor::{self, Span};
use crate::layer::{Actor, Answer, Link, LogEntry, Note, Private, Question, QuestionStatus};
use crate::mask::{self, View};

const LOC: &str = "Review › Agent";

/// The commands, registered by [`crate::install`].
pub fn specs() -> Vec<CommandSpec> {
    let c = CommandSpec::new;
    vec![
        // the reader's side
        c("agent.ask", "Ask the Agent", LOC, ask)
            .params(r#"{"text": string, "paragraph"?: number, "quote"?: string} — on the selection unless a paragraph/quote is given"#)
            .pure(),
        c("agent.seen", "Mark Answer Read", LOC, seen).params(r#"{"id": string}"#).pure(),
        c("agent.private", "Mark as Client Data", LOC, private)
            .params(r#"{"paragraph"?: number, "quote"?: string, "label"?: string (placeholder name, e.g. "client.name"), "on"?: bool (default true)} — the agent sees {{label}} there"#)
            .pure(),
        c("agent.removeNote", "Remove Agent Note", LOC, remove_note).params(r#"{"id": string}"#).pure(),
        c("agent.render", "Show Template For Client", LOC, render)
            .params(r#"{"client": string (CRM id) | null (the template)} — renders the template with the client's data, read only"#)
            .pure(),
        c("agent.clients", "Template Clients", LOC, clients).params("{} → the CRM's clients (people only)").pure(),
        c("agent.openDemo", "Open Agent Demo Template", LOC, open_demo).params("{} — opens the engagement-letter template").pure(),
        // the agent's side
        c("agent.context", "Agent Context", LOC, context).params("{} → outline, client-data placeholders, template vocabulary, open questions, recent history").pure(),
        c("agent.read", "Agent Read", LOC, read).params(r#"{"from"?: number, "count"?: number (default 400)} → paragraphs (masked)"#).pure(),
        c("agent.questions", "Agent Questions", LOC, questions).params(r#"{"status"?: "pending"|"working"|"answered"|"open"|"all"}"#).pure(),
        c("agent.claim", "Take Question", LOC, claim).params(r#"{"id": string} — marks it as being worked on"#).pure(),
        c("agent.answer", "Answer Question", LOC, answer).params(r#"{"id": string, "text": string, "links"?: [{"label", "href", "summary"?}]}"#).pure(),
        c("agent.note", "Add Agent Note", LOC, note)
            .params(r#"{"paragraph"?: number, "quote"?: string, "text": string, "title"?: string, "kind"?: "explanation"|"faq"|"assumption"|"source", "links"?: [{"label", "href", "summary"?}]}"#)
            .pure(),
        c("agent.decision", "Log Decision", LOC, decision).params(r#"{"text": string, "paragraph"?: number, "quote"?: string, "source"?: string}"#).pure(),
        c("agent.layer", "Agent Layer", LOC, layer_cmd).params("{} → questions, notes, client-data ranges, history (masked)").pure(),
        c("agent.documents", "Agent Documents", LOC, documents).params("{} → the open document as the AItoCraft registry describes it").pure(),
        c("agent.attach", "Attach a Conversation", LOC, attach)
            .params(r#"{"document"?: id, "label": string} → {listener, document, title, app}: a conversation that listens for this document's questions"#)
            .pure(),
        c("agent.detach", "Detach a Conversation", LOC, detach).params(r#"{"listener": string}"#).pure(),
        c("agent.ping", "Check the Agent Connection", LOC, ping)
            .params(r#"{} — asks "is anyone there?" (a system question with kind ping: never shown, never saved)"#)
            .pure(),
        c("agent.exec", "Run Command as Agent", LOC, exec)
            .params(r#"{"command": string, "params"?: object, "at"?: {"paragraph"?: number, "quote"?: string, "occurrence"?: number}, "reason": string, "source"?: string} — a changing command, logged as the agent's"#)
            .pure(),
    ]
}

// ---------------------------------------------------------------- params

fn bad(msg: impl Into<String>) -> CmdError {
    CmdError::Params(msg.into())
}

fn text_p(v: &Json, k: &str, max: usize) -> Result<String, CmdError> {
    let t = p::str(v, k).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(format!("`{k}` (string) is required")))?;
    Ok(t.chars().take(max).collect())
}

fn opt_text(v: &Json, k: &str, max: usize) -> String {
    p::str(v, k).unwrap_or("").trim().chars().take(max).collect()
}

/// Is the caller the agent (the MCP surface says so, or `agent.exec` is running)?
fn is_agent(v: &Json) -> bool {
    p::str(v, "actor") == Some("agent") || in_agent_exec()
}

fn links_p(v: &Json) -> Vec<Link> {
    v.get("links")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .take(50)
                .filter_map(|x| serde_json::from_value::<Link>(x.clone()).ok())
                .map(|mut l| {
                    l.label = l.label.chars().take(300).collect();
                    l.href = l.href.chars().take(2_000).collect();
                    l.summary = l.summary.chars().take(2_000).collect();
                    l
                })
                .filter(|l| !l.href.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// `paragraph` / `quote` / `occurrence` params, in the agent's (masked) view.
fn place_in(view: &View, v: &Json) -> Result<Option<Span>, CmdError> {
    let para = p::u64(v, "paragraph").map(|n| n.min(usize::MAX as u64) as usize);
    let quote = p::str(v, "quote");
    if para.is_none() && quote.is_none() {
        return Ok(None);
    }
    let occurrence = p::u64(v, "occurrence").unwrap_or(0).min(10_000) as usize;
    view.find(para, quote, occurrence).map(Some).map_err(bad)
}

/// The reader's selection as a range in one paragraph (the first one it touches); a caret takes
/// its whole paragraph.
fn selection_span(s: &Session) -> Result<Span, CmdError> {
    let (a, b) = s.sel.ordered();
    if a.story != StoryRef::Body {
        return Err(bad("select text in the body of the document first"));
    }
    let paras = anchor::paragraphs(&s.doc);
    let idx = paras.iter().position(|x| *x == a.path).ok_or_else(|| bad("the selection is not in a paragraph"))?;
    let len = anchor::para_text(&s.doc, &a.path).map_or(0, str::len);
    let (start, end) = if a == b {
        (0, len)
    } else if b.path == a.path {
        (a.off, b.off)
    } else {
        (a.off, len)
    };
    Ok(Span { para: idx, path: a.path.clone(), start: start.min(len), end: end.min(len) })
}

/// Where a reader's command applies: the given paragraph/quote (real text), else the selection.
fn human_place(s: &Session, v: &Json) -> Result<Span, CmdError> {
    let plain = View::new(&s.doc, &Default::default(), &[]);
    match place_in(&plain, v)? {
        Some(sp) => Ok(sp),
        None => selection_span(s),
    }
}

/// Where a command applies, for whoever calls it.
fn place(s: &Session, v: &Json) -> Result<Span, CmdError> {
    if is_agent(v) {
        let view = crate::view(s);
        place_in(&view, v)?.ok_or_else(|| bad("give `paragraph` (number from read_document) and/or `quote` (text as read)"))
    } else {
        human_place(s, v)
    }
}

/// Adds an anchor for `span` (in the document on screen); returns its id.
fn add_anchor(s: &mut Session, span: &Span, prefix: &str) -> Result<String, CmdError> {
    let doc = if is_agent_doc_template(s) { crate::agent_doc(s).clone() } else { s.doc.clone() };
    let id = crate::update(s, |l| l.fresh_id(prefix));
    let a = anchor::make(&doc, id.clone(), &span.path, span.start, span.end).ok_or_else(|| bad("no such paragraph"))?;
    crate::update(s, |l| l.anchors.push(a));
    Ok(id)
}

/// While a template preview is shown, the agent's anchors are on the template.
fn is_agent_doc_template(s: &Session) -> bool {
    in_agent_exec() || (crate::render_state(s).is_some() && AGENT_CALL.with(Cell::get))
}

thread_local! {
    /// > 0 while `agent.exec` runs a command: the history records it as the agent's.
    static AGENT_DEPTH: Cell<u32> = const { Cell::new(0) };
    /// The reason and source of the command `agent.exec` is running.
    static PENDING: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    /// An agent-side command is running (anchors go on the template).
    static AGENT_CALL: Cell<bool> = const { Cell::new(false) };
}

/// Whether `agent.exec` is running a command now.
pub fn in_agent_exec() -> bool {
    AGENT_DEPTH.with(Cell::get) > 0
}

fn as_agent<R>(v: &Json, f: impl FnOnce() -> R) -> R {
    let agent = is_agent(v);
    let was = AGENT_CALL.with(|c| c.replace(agent));
    let r = f();
    AGENT_CALL.with(|c| c.set(was));
    r
}

/// A paragraph index as the agent knows it, for an anchor id (in the agent's document).
fn where_of(s: &Session, view: &View, anchor_id: &str) -> Json {
    let l = crate::layer(s);
    let Some(a) = l.anchor(anchor_id) else { return Json::Null };
    let doc = crate::agent_doc(s);
    let paras: Vec<Path> = view.paras.iter().map(|m| m.path.clone()).collect();
    match anchor::resolve(doc, &paras, a) {
        Some(sp) => {
            let mp = view.para(sp.para);
            json!({"paragraph": sp.para, "quote": mp.map(|m| m.masked_slice(sp.start, sp.end)), "paragraphText": mp.map(|m| m.text.as_str())})
        }
        None => json!({"paragraph": Json::Null, "quote": view.scrub(&a.quote), "lost": true}),
    }
}

// ---------------------------------------------------------------- reader's side

fn ask(s: &mut Session, v: &Json) -> CmdResult {
    let text = text_p(v, "text", 20_000)?;
    let span = as_agent(v, || place(s, v))?;
    let a = as_agent(v, || add_anchor(s, &span, "a"))?;
    let agent = is_agent(v);
    let asked_by = if agent { "agent".to_string() } else { s.author.clone() };
    let id = crate::update(s, |l| {
        let id = l.fresh_id("q");
        l.log(LogEntry {
            at: crate::now_ms(),
            actor: if agent { Actor::Agent } else { Actor::Human },
            kind: "question".into(),
            text: format!("Asked “{text}” · ¶{}", span.para),
            command: "agent.ask".into(),
            anchor: Some(a.clone()),
            source: id.clone(),
            who: asked_by.clone(),
            ..Default::default()
        });
        l.questions.push(Question {
            id: id.clone(),
            anchor: a,
            text,
            status: QuestionStatus::Pending,
            asked_at: crate::now_ms(),
            asked_by,
            answer: None,
            seen: false,
        });
        id
    });
    Ok(json!({"id": id, "paragraph": span.para}))
}

fn seen(s: &mut Session, v: &Json) -> CmdResult {
    let id = text_p(v, "id", 100)?;
    let ok = crate::update(s, |l| l.questions.iter_mut().find(|q| q.id == id).map(|q| q.seen = true).is_some());
    if ok { Ok(json!({"id": id})) } else { Err(bad(format!("no question `{id}`"))) }
}

fn private(s: &mut Session, v: &Json) -> CmdResult {
    let on = p::bool(v, "on").unwrap_or(true);
    let agent = is_agent(v);
    if agent && !on {
        return Err(bad("only a person can lift the client-data mark"));
    }
    if crate::render_state(s).is_some() {
        return Err(bad("show the template (not a client's rendering) to mark client data"));
    }
    let span = as_agent(v, || place(s, v))?;
    if on {
        if span.start >= span.end {
            return Err(bad("select the client data to mark (an empty range can't hold any)"));
        }
        let quote = anchor::para_text(&s.doc, &span.path).and_then(|t| t.get(span.start..span.end)).unwrap_or("").to_string();
        let l = crate::layer(s);
        // The same text keeps the same placeholder.
        let same = l.private.iter().find(|p| l.anchor(&p.anchor).is_some_and(|a| a.quote == quote)).map(|p| p.label.clone());
        let label = match (opt_text(v, "label", 80), same) {
            (given, _) if !given.is_empty() => clean_label(&given),
            (_, Some(existing)) => existing,
            _ => format!("client.data{}", l.private.len() + 1),
        };
        let a = add_anchor(s, &span, "a")?;
        crate::update(s, |l| l.private.push(Private { anchor: a, label: label.clone() }));
        Ok(json!({"paragraph": span.para, "placeholder": mask::placeholder(&label)}))
    } else {
        let l = crate::layer(s);
        let paras = anchor::paragraphs(&s.doc);
        let hits: Vec<String> = l
            .private
            .iter()
            .filter(|p| {
                l.anchor(&p.anchor)
                    .and_then(|a| anchor::resolve(&s.doc, &paras, a))
                    .is_some_and(|x| x.path == span.path && x.start < span.end.max(span.start + 1) && span.start < x.end)
            })
            .map(|p| p.anchor.clone())
            .collect();
        crate::update(s, |l| l.private.retain(|p| !hits.contains(&p.anchor)));
        Ok(json!({"paragraph": span.para, "removed": hits.len()}))
    }
}

/// A placeholder name: letters, digits, `.`, `_`, `-`.
fn clean_label(s: &str) -> String {
    let t: String = s
        .trim()
        .trim_start_matches("{{")
        .trim_end_matches("}}")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(60)
        .collect();
    if t.is_empty() { "client.data".into() } else { t }
}

fn remove_note(s: &mut Session, v: &Json) -> CmdResult {
    let id = text_p(v, "id", 100)?;
    let ok = crate::update(s, |l| {
        let n = l.notes.len();
        l.notes.retain(|x| x.id != id);
        n != l.notes.len()
    });
    if ok { Ok(json!({"id": id})) } else { Err(bad(format!("no note `{id}`"))) }
}

fn render(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("only a person can show a client's data; you work on the template"));
    }
    let client = p::str(v, "client").filter(|c| !c.is_empty());
    crate::set_render(s, client).map_err(bad)?;
    Ok(json!({"client": client}))
}

fn clients(_s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("client data is not available to the agent: use the placeholders in agent.context"));
    }
    Ok(Json::Array(crate::crm::clients().iter().map(|c| json!({"id": c.get("id"), "name": c.get("name")})).collect()))
}

fn open_demo(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("only a person can replace the document"));
    }
    s.set_document(crate::sample::engagement_letter());
    s.file_name = Some("Engagement letter (template).docx".into());
    crate::ensure(s);
    Ok(json!({}))
}

// ---------------------------------------------------------------- agent's side

/// Agent-facing results: scrubbed of client data (unless a person asks for them unmasked).
fn masked(view: &View, v: &Json, mut out: Json) -> CmdResult {
    if is_agent(v) || p::bool(v, "masked") != Some(false) {
        view.scrub_json(&mut out);
    }
    Ok(out)
}

fn context(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let view = crate::view(s);
    let l = crate::layer(s);
    let doc = crate::agent_doc(s);
    let outline: Vec<Json> = view
        .paras
        .iter()
        .filter(|m| m.style.as_deref().is_some_and(|st| st.starts_with("Heading") || st == "Title"))
        .take(500)
        .map(|m| json!({"paragraph": m.index, "style": m.style, "text": m.text}))
        .collect();
    let private: Vec<Json> = view.private.iter().map(|(sp, label)| json!({"paragraph": sp.para, "placeholder": mask::placeholder(label)})).collect();
    let template = crate::template::is_template(doc);
    let out = json!({
        "title": doc.core.title,
        "file": s.file_name,
        "paragraphs": view.paras.len(),
        "outline": outline,
        "clientData": private,
        "privacyNote": "Text marked as client data reads as {{placeholder}} everywhere you look, and you can't write into it. Refer to it by its placeholder; never try to find the real value.",
        "template": if template { crate::crm::vocabulary() } else { Json::Null },
        "preview": if crate::render_state(s).is_some() { json!("the reader is looking at the template rendered for a client; you see and edit the template") } else { Json::Null },
        "openQuestions": l.questions.iter().filter(|q| q.status != QuestionStatus::Answered).count(),
        "notes": l.notes.len(),
        "history": l.log.iter().rev().take(20).collect::<Vec<_>>(),
    });
    masked(&view, v, out)
}

fn read(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let view = crate::view(s);
    let from = p::u64(v, "from").unwrap_or(0).min(usize::MAX as u64) as usize;
    let count = p::u64(v, "count").unwrap_or(400).clamp(1, 2_000) as usize;
    let paras: Vec<Json> = view
        .paras
        .iter()
        .skip(from)
        .take(count)
        .map(|m| {
            let mut o = json!({"paragraph": m.index, "text": m.text});
            if let Some(st) = &m.style {
                o["style"] = json!(st);
            }
            if m.in_table {
                o["inTable"] = json!(true);
            }
            if !m.private.is_empty() {
                o["clientData"] = json!(m.private);
            }
            o
        })
        .collect();
    let out = json!({"total": view.paras.len(), "from": from, "paragraphs": paras});
    masked(&view, v, out)
}

fn question_json(s: &Session, view: &View, q: &Question) -> Json {
    let mut o = serde_json::to_value(q).unwrap_or(Json::Null);
    if let Some(m) = o.as_object_mut() {
        m.remove("anchor");
        if let Json::Object(w) = where_of(s, view, &q.anchor) {
            m.extend(w);
        }
    }
    o
}

fn status_matches(q: &Question, want: &str) -> bool {
    match want {
        "pending" => q.status == QuestionStatus::Pending,
        "working" => q.status == QuestionStatus::Working,
        "answered" => q.status == QuestionStatus::Answered,
        "open" => q.status != QuestionStatus::Answered,
        _ => true,
    }
}

/// The agent's calls are refused while the connection is off; a call naming another document is
/// refused too (a WordCraft window holds one document).
fn gate(v: &Json) -> Result<(), CmdError> {
    if is_agent(v) && crate::presence::mode() == crate::presence::Mode::Off {
        return Err(CmdError::Disabled("the agent connection is off in WordCraft (Agent tab ▸ Connection ▸ Mode)".into()));
    }
    Ok(())
}

/// `document` (a registry id), when given, must be this one.
fn doc_check(s: &Session, v: &Json) -> Result<(), CmdError> {
    match p::str(v, "document") {
        Some(d) if d != crate::document_id(s) => Err(CmdError::Params(format!("document {d} is not open in {}", crate::APP_TITLE))),
        _ => Ok(()),
    }
}

/// A call carrying `listener` refreshes it; an unknown one must attach again.
fn listener_check(v: &Json) -> Result<Option<crate::presence::Listener>, CmdError> {
    match p::str(v, "listener") {
        Some(l) => crate::presence::touch(l).map(Some).ok_or_else(|| {
            CmdError::Params(format!("listener {l} is not attached (dropped after 90 s without a call, or WordCraft restarted): attach again"))
        }),
        None => Ok(None),
    }
}

fn documents(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let d = crate::document_json(s);
    masked(&crate::view(s), v, json!([d]))
}

fn attach(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let label = p::str(v, "label").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("an agent").to_string();
    let l = crate::presence::attach(&crate::document_id(s), &label);
    let out = json!({"listener": l.listener, "document": l.document, "title": crate::document_title(s), "app": crate::APP});
    masked(&crate::view(s), v, out)
}

fn detach(_s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    let id = text_p(v, "listener", 100)?;
    Ok(json!({"listener": id, "detached": crate::presence::detach(&id)}))
}

fn ping_matches(p: &crate::presence::Ping, want: &str) -> bool {
    match want {
        "pending" | "working" | "answered" => p.status() == want,
        "open" => p.answered_at.is_none(),
        _ => true,
    }
}

fn questions(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    listener_check(v)?;
    let l = crate::layer(s);
    let want = p::str(v, "status").unwrap_or("all");
    // The connection check first: an agent answers it at once.
    // A check nobody answered in time has done its job ("No answer"): it goes to no one later.
    let now = crate::now_ms();
    let mut list: Vec<Json> = crate::ping(s)
        .filter(|p| p.answered_at.is_some() || now.saturating_sub(p.asked_at) < crate::presence::PING_TIMEOUT_MS)
        .filter(|p| ping_matches(p, want))
        .map(|p| json!({"id": p.id, "kind": "ping", "text": crate::presence::PING_TEXT, "status": p.status(), "askedAt": p.asked_at}))
        .into_iter()
        .collect();
    let view = crate::view(s);
    list.extend(l.questions.iter().filter(|q| status_matches(q, want)).map(|q| question_json(s, &view, q)));
    // Every question says which document it is in (the hub routes by it).
    let (doc, title) = (crate::document_id(s), crate::document_title(s));
    for q in &mut list {
        if let Some(o) = q.as_object_mut() {
            o.insert("document".into(), json!(doc));
            o.insert("documentTitle".into(), json!(title));
        }
    }
    masked(&view, v, Json::Array(list))
}

fn ping(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("the connection check is the reader's"));
    }
    crate::ensure(s);
    let p = crate::presence::Ping { id: format!("ping{}", crate::now_ms()), asked_at: crate::now_ms(), ..Default::default() };
    let id = p.id.clone();
    if let Some(st) = s.ext.get_mut::<crate::State>() {
        st.ping = Some(p);
    }
    Ok(json!({"id": id}))
}

/// Claims or answers the connection check, if `id` is it; `by` is the listener that did.
fn ping_reply(s: &mut Session, id: &str, answer: bool, by: Option<&crate::presence::Listener>) -> bool {
    let Some(st) = s.ext.get_mut::<crate::State>() else { return false };
    match st.ping.as_mut() {
        Some(p) if p.id == id => {
            p.claimed = true;
            if answer && p.answered_at.is_none() {
                p.answered_at = Some(crate::now_ms());
                p.answered_by = by.map(|l| l.label.clone()).unwrap_or_default();
            }
            true
        }
        _ => false,
    }
}

fn claim(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let by = listener_check(v)?;
    let id = text_p(v, "id", 100)?;
    if ping_reply(s, &id, false, by.as_ref()) {
        return Ok(json!({"id": id}));
    }
    let r = crate::update(s, |l| match l.questions.iter_mut().find(|q| q.id == id) {
        Some(q) if q.status == QuestionStatus::Pending => {
            q.status = QuestionStatus::Working;
            Ok(())
        }
        Some(q) => Err(format!("question `{id}` is {:?}", q.status).to_lowercase()),
        None => Err(format!("no question `{id}`")),
    });
    r.map(|()| json!({"id": id})).map_err(bad)
}

fn answer(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let by = listener_check(v)?;
    let id = text_p(v, "id", 100)?;
    let text = text_p(v, "text", 20_000)?;
    if ping_reply(s, &id, true, by.as_ref()) {
        return Ok(json!({"id": id, "check": "answered"}));
    }
    let links = links_p(v);
    let ok = crate::update(s, |l| {
        let Some(q) = l.questions.iter_mut().find(|q| q.id == id) else { return false };
        q.status = QuestionStatus::Answered;
        q.seen = false;
        q.answer = Some(Answer { text, links, at: crate::now_ms() });
        let (anchor, qtext) = (q.anchor.clone(), q.text.clone());
        l.log(LogEntry {
            at: crate::now_ms(),
            actor: Actor::Agent,
            kind: "decision".into(),
            text: format!("Answered “{qtext}”"),
            command: "agent.answer".into(),
            anchor: Some(anchor),
            source: id.clone(),
            who: "agent".into(),
            ..Default::default()
        });
        true
    });
    if ok { Ok(json!({"id": id})) } else { Err(bad(format!("no question `{id}`"))) }
}

fn note(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let text = text_p(v, "text", 20_000)?;
    let span = as_agent(v, || place(s, v))?;
    let kind = match p::str(v, "kind") {
        Some(k @ ("explanation" | "faq" | "assumption" | "source")) => k.to_string(),
        _ => "explanation".into(),
    };
    let title = opt_text(v, "title", 200);
    let links = links_p(v);
    let author = if is_agent(v) { Actor::Agent } else { Actor::Human };
    let a = as_agent(v, || add_anchor(s, &span, "a"))?;
    let id = crate::update(s, |l| {
        let id = l.fresh_id("n");
        l.notes.push(Note { id: id.clone(), anchor: a, kind, title, text, links, author, at: crate::now_ms() });
        id
    });
    Ok(json!({"id": id, "paragraph": span.para}))
}

fn decision(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let text = text_p(v, "text", 4_000)?;
    let anchor = if p::u64(v, "paragraph").is_some() || p::str(v, "quote").is_some() {
        let span = as_agent(v, || place(s, v))?;
        Some(as_agent(v, || add_anchor(s, &span, "a"))?)
    } else {
        None
    };
    let source = opt_text(v, "source", 2_000);
    let agent = is_agent(v);
    let who = if agent { "agent".to_string() } else { s.author.clone() };
    crate::update(s, |l| {
        l.log(LogEntry {
            at: crate::now_ms(),
            actor: if agent { Actor::Agent } else { Actor::Human },
            kind: "decision".into(),
            text,
            anchor,
            source,
            who,
            ..Default::default()
        })
    });
    Ok(json!({}))
}

/// The layer, with each anchor's place (masked for the agent).
pub fn layer_json(s: &Session, v: &Json) -> CmdResult {
    let view = crate::view(s);
    let l = crate::layer(s);
    let questions: Vec<Json> = l.questions.iter().map(|q| question_json(s, &view, q)).collect();
    let notes: Vec<Json> = l
        .notes
        .iter()
        .map(|n| {
            let mut o = serde_json::to_value(n).unwrap_or(Json::Null);
            if let Some(m) = o.as_object_mut() {
                m.remove("anchor");
                if let Json::Object(w) = where_of(s, &view, &n.anchor) {
                    m.extend(w);
                }
            }
            o
        })
        .collect();
    let private: Vec<Json> = view.private.iter().map(|(sp, label)| json!({"paragraph": sp.para, "placeholder": mask::placeholder(label)})).collect();
    let out = json!({"questions": questions, "notes": notes, "clientData": private, "history": l.log});
    masked(&view, v, out)
}

fn layer_cmd(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    layer_json(s, v)
}

// ---------------------------------------------------------------- acting as the agent

/// Commands the agent may not run through `agent.exec`: they read text out of the document or
/// the clipboard, search the real text, touch files, or would undo the reader's work.
fn refused(id: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "agent.",
        "file.",
        "document.",
        "ui.",
        "tools.",
        "mailings.",
        "edit.find",
        "edit.replace",
        "edit.paste",
        "edit.copy",
        "edit.cut",
        "edit.undo",
        "edit.redo",
        "edit.repeat",
        "edit.goto",
        "edit.goBack",
        "edit.clipboard",
        "edit.advancedFind",
        "edit.deleteClipboard",
        "edit.clearClipboard",
        "select.text",
        "select.similar",
    ];
    PREFIXES.iter().any(|p| id.starts_with(p))
}

/// The client data's texts and how often each occurs: an edit must leave both alone.
fn private_fingerprint(s: &Session) -> Vec<(String, usize, bool)> {
    let l = crate::layer(s);
    let paras = anchor::paragraphs(&s.doc);
    let whole: String = paras.iter().filter_map(|p| anchor::para_text(&s.doc, p)).collect::<Vec<_>>().join("\n");
    l.private
        .iter()
        .filter_map(|p| l.anchor(&p.anchor))
        .map(|a| {
            let still = anchor::resolve(&s.doc, &paras, a)
                .and_then(|sp| anchor::para_text(&s.doc, &sp.path).and_then(|t| t.get(sp.start..sp.end)).map(str::to_string));
            let n = if a.quote.is_empty() { 0 } else { whole.matches(a.quote.as_str()).count() };
            (a.quote.clone(), n, still.as_deref() == Some(a.quote.as_str()))
        })
        .collect()
}

fn exec(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let command = text_p(v, "command", 200)?;
    let params = v.get("params").cloned().filter(Json::is_object).unwrap_or_else(|| json!({}));
    let Some(spec) = s.registry.get(&command).cloned() else { return Err(CmdError::Unknown(command)) };
    let positional = command.starts_with("select.") || command.starts_with("caret.");
    if refused(&command) || !(spec.mutates || positional) {
        return Err(bad(format!(
            "`{command}` can't be run by the agent: it reads or searches the document's real text, the clipboard or files. Read with read_document; point at text with `at`"
        )));
    }
    let reason = text_p(v, "reason", 4_000).map_err(|_| bad("`reason` is required: say why you are changing the document"))?;
    let source = opt_text(v, "source", 2_000);
    // While a client's rendering is shown, the agent edits the template underneath.
    let swapped = crate::template_in(s);
    let r = exec_on(s, &command, params, v, reason, source);
    crate::template_out(s, swapped);
    r
}

/// Runs `command` for the agent, leaving the reader's selection where it was.
fn exec_on(s: &mut Session, command: &str, params: Json, v: &Json, reason: String, source: String) -> CmdResult {
    let reader = s.sel.clone();
    let r = exec_at(s, command, params, v, reason, source);
    let (a, f) = (s.doc.clamp(&reader.anchor), s.doc.clamp(&reader.focus));
    s.sel = Selection { anchor: a, focus: f };
    r
}

fn exec_at(s: &mut Session, command: &str, params: Json, v: &Json, reason: String, source: String) -> CmdResult {
    let view = crate::view(s);
    let sel = match v.get("at").filter(|a| a.is_object()) {
        Some(at) => {
            let span = place_in(&view, at)?.ok_or_else(|| bad("`at` needs `paragraph` and/or `quote`"))?;
            Selection { anchor: span.start_pos(), focus: span.end_pos() }
        }
        None => s.sel.clone(),
    };
    // Nothing touching client data.
    let (a, b) = sel.ordered();
    if a.story == StoryRef::Body {
        let paras = anchor::paragraphs(&s.doc);
        for (i, path) in paras.iter().enumerate() {
            if *path < a.path || *path > b.path {
                continue;
            }
            let len = anchor::para_text(&s.doc, path).map_or(0, str::len);
            let start = if *path == a.path { a.off } else { 0 };
            let end = if *path == b.path { b.off } else { len };
            if view.touches_private(&Span { para: i, path: path.clone(), start, end }) {
                return Err(bad("that range holds client data ({{…}}): the agent can't write there"));
            }
        }
    }
    s.sel = sel;
    let before = private_fingerprint(s);
    let log_len = crate::layer(s).log.len();
    let snap = s.edit_snapshot();
    PENDING.with(|p| *p.borrow_mut() = Some((reason, source)));
    AGENT_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
    let r = s.run(command, &params);
    AGENT_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    PENDING.with(|p| p.borrow_mut().take());
    if r.is_ok() && private_fingerprint(s) != before {
        s.restore(snap);
        crate::refresh_anchors(s);
        crate::update(s, |l| l.log.truncate(log_len));
        return Err(bad("that change would alter client data ({{…}}), so it was taken back"));
    }
    // Only success is reported: a command's result can carry text.
    r.map(|_| json!({"ok": true, "command": command}))
}

/// Records a change in the history (called by the observer after a changing command). People's
/// own edits are recorded once the document has an agent layer (a plain document stays plain).
pub(crate) fn log_change(s: &mut Session, id: &str, _params: &Json) {
    if id.starts_with("agent.") {
        return;
    }
    let agent = in_agent_exec();
    if !agent && crate::layer(s).is_empty() {
        return;
    }
    let label = s.registry.get(id).map(|c| c.label.trim_end_matches('…').to_string()).unwrap_or_else(|| id.to_string());
    let paras = anchor::paragraphs(&s.doc);
    let at = paras.iter().position(|p| s.sel.focus.story == StoryRef::Body && *p == s.sel.focus.path);
    let (reason, source) = if agent { PENDING.with(|p| p.borrow().clone()).unwrap_or_default() } else { Default::default() };
    let place = at.map(|i| format!(" · ¶{i}")).unwrap_or_default();
    let text = if reason.is_empty() { format!("{label}{place}") } else { format!("{label}{place}: {reason}") };
    let who = if agent { "agent".to_string() } else { s.author.clone() };
    let dirty = s.dirty;
    crate::update(s, |l| {
        l.log(LogEntry {
            at: crate::now_ms(),
            actor: if agent { Actor::Agent } else { Actor::Human },
            kind: "change".into(),
            text,
            command: id.into(),
            source,
            who,
            ..Default::default()
        })
    });
    s.dirty = dirty;
}
