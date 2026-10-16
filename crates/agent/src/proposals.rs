//! Proposals instead of direct edits (the same surface in GridCraft and DeckCraft).
//!
//! In a topic, the agent changes the document with the ordinary `run_command` plus `topic` (and
//! `option`, "A" by default): the command doesn't touch the document. It runs on the topic's draft
//! (the accepted document with the option's earlier commands replayed), which checks it, and is
//! recorded in the option's command list. An option has a title and a why (`describe_option`);
//! the agent can drop one or all (`discard_proposal`).
//!
//! The person sees "The agent proposes N changes", the options, and a preview in the document
//! (the draft with its changes tracked: inserted, deleted, formatting changed, new tables). **Apply
//! option X** replays the option's commands on the document as one undo step, the person's
//! action, and adds a decision note "chose X: <title>"; **Reject** (with an optional comment, which
//! reaches the agent as a follow-up question in the topic) closes it. If a step no longer fits the
//! document, nothing is applied and the proposal is out of date: the agent redoes it. Applying one
//! re-checks the others, so overlapping ones go out of date too. The agent hears of it all through
//! events (`proposal_applied`, `proposal_rejected`, `proposal_outdated`).
//!
//! A step's target is kept as `{paragraph, quote, occurrence}` in the agent's (masked) view of the
//! draft it ran on, plus the paragraph's text. Replayed on a document the person has edited since,
//! it is found again by its quote (or paragraph text) nearest to where it was.

use serde_json::{Value as Json, json};
use wordcraft_doc::Document;
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::commands::{bad, doc_check, gate, is_agent, opt_text, text_p};
use crate::layer::{Actor, EditProposal, Layer, LogEntry, Note, ProposalOption, ProposalState, Question, QuestionStatus, Step};
use crate::mask::View;
use crate::topics;
use crate::undo::{self, Op};

const LOC: &str = "Review › Agent";

pub fn specs() -> Vec<CommandSpec> {
    let c = CommandSpec::new;
    vec![
        // the agent's
        c("agent.describeOption", "Describe Proposal Option", LOC, describe_option)
            .params(r#"{"topic": id|number, "option": "A", "title": string, "why": string}"#)
            .pure(),
        c("agent.discardProposal", "Discard Proposal", LOC, discard).params(r#"{"topic": id|number, "option"?: "A" (else all of it)}"#).pure(),
        c("agent.events", "Agent Events", LOC, events).params(r#"{"after"?: number} → proposal events after that sequence number"#).pure(),
        // the person's
        c("agent.applyProposal", "Apply Proposal", LOC, apply).params(r#"{"topic": id|number, "option"?: "A"} — one undo step"#),
        c("agent.rejectProposal", "Reject Proposal", LOC, reject)
            .params(r#"{"topic": id|number, "comment"?: string} — the comment goes to the agent as a follow-up"#),
        c("agent.previewProposal", "Preview Proposal", LOC, preview)
            .params(r#"{"topic"?: id|number, "option"?: "A"} — show the option's changes tracked in the document (read only); no topic: stop"#)
            .pure(),
    ]
}

fn topic_of(s: &Session, v: &Json) -> Result<String, CmdError> {
    let raw = match v.get("topic") {
        Some(Json::String(t)) => t.clone(),
        Some(Json::Number(n)) => n.to_string(),
        _ => return Err(bad("`topic` (id or number) is required")),
    };
    topics::find(s, &raw).ok_or_else(|| bad(format!("no topic `{raw}`")))
}

/// "A", "b", "short" → "A", "B", "SHORT" (letters and digits, at most 12).
fn option_of(v: &Json) -> Result<Option<String>, CmdError> {
    let Some(o) = p::str(v, "option") else { return Ok(None) };
    let o = o.trim();
    if o.is_empty() || o.len() > 12 || !o.chars().all(|c| c.is_alphanumeric()) {
        return Err(bad("`option` is a short name: A, B, C…"));
    }
    Ok(Some(o.to_uppercase()))
}

// ---------------------------------------------------------------- drafts

/// A scratch session on `base` with the layer's marks (so privacy guards hold there too).
fn scratch(base: &Document, l: &Layer, tracked: bool) -> Session {
    let mut doc = base.clone();
    crate::layer::write_into(&mut doc, l);
    let mut sc = Session::new(doc);
    sc.author = "Your agent".into();
    if tracked {
        sc.doc.settings.track_changes = true;
    }
    sc
}

/// Finds a step's target again in `view`: where it was if it is still there, else the nearest
/// match of its quote (or its paragraph's text).
pub(crate) fn locate(view: &View, at: &Json, para_text: &str) -> Option<Json> {
    let mut found = locate_start(view, at, para_text)?;
    // The rest of `at` goes along: `through` moved as much as the start did.
    let was = p::u64(at, "paragraph")?;
    let now = p::u64(&found, "paragraph")?;
    if let Some(t) = p::u64(at, "through") {
        found["through"] = json!((t + now).saturating_sub(was).min(view.paras.len().saturating_sub(1) as u64));
    }
    for k in ["throughQuote", "caret"] {
        if let Some(x) = at.get(k) {
            found[k] = x.clone();
        }
    }
    Some(found)
}

fn locate_start(view: &View, at: &Json, para_text: &str) -> Option<Json> {
    let para = p::u64(at, "paragraph").map(|n| n.min(usize::MAX as u64) as usize)?;
    let occ = p::u64(at, "occurrence").unwrap_or(0).min(10_000) as usize;
    match p::str(at, "quote").filter(|q| !q.is_empty()) {
        Some(q) => {
            if view.find(Some(para), Some(q), occ).is_ok() {
                return Some(json!({"paragraph": para, "quote": q, "occurrence": occ}));
            }
            let mut best: Option<(usize, usize)> = None;
            for k in 0..500 {
                let Ok(sp) = view.find(None, Some(q), k) else { break };
                let d = sp.para.abs_diff(para);
                if best.is_none_or(|(bp, _)| d < bp.abs_diff(para)) {
                    best = Some((sp.para, sp.start));
                }
            }
            let (bp, start) = best?;
            let occ = (0..500).take_while(|j| view.find(Some(bp), Some(q), *j).is_ok_and(|sp| sp.start < start)).count();
            Some(json!({"paragraph": bp, "quote": q, "occurrence": occ}))
        }
        None => {
            if para_text.is_empty() || view.para(para).is_some_and(|m| m.text == para_text) {
                return view.para(para).map(|_| json!({"paragraph": para}));
            }
            let found = view.paras.iter().filter(|m| m.text == para_text).min_by_key(|m| m.index.abs_diff(para))?;
            Some(json!({"paragraph": found.index}))
        }
    }
}

/// Replays steps on `sc`; the first that doesn't fit, with why.
fn replay(sc: &mut Session, steps: &[Step]) -> Result<(), (usize, String)> {
    for (i, st) in steps.iter().enumerate() {
        let view = crate::view(sc);
        let at = locate(&view, &st.at, &st.para_text).ok_or((i, "its text is no longer in the document".to_string()))?;
        crate::commands::exec_at(sc, &st.command, st.params.clone(), &json!({"at": at, "actor": "agent"}), st.reason.clone(), st.source.clone())
            .map_err(|e| (i, e.to_string()))?;
    }
    Ok(())
}

fn steps_of(l: &Layer, topic: &str, option: &str) -> Vec<Step> {
    l.topic(topic)
        .and_then(|t| t.edits.as_ref())
        .and_then(|e| e.options.iter().find(|o| o.option == option))
        .map(|o| o.steps.clone())
        .unwrap_or_default()
}

/// The document with an option's changes (tracked: as revisions, for the preview).
pub fn draft(s: &Session, topic: &str, option: &str, tracked: bool) -> Result<Document, String> {
    let l = crate::layer(s);
    let mut sc = scratch(crate::agent_doc(s), &l, tracked);
    replay(&mut sc, &steps_of(&l, topic, option)).map_err(|(i, e)| format!("step {} no longer fits: {e}", i + 1))?;
    sc.doc.settings.track_changes = false;
    Ok(sc.doc)
}

/// What a step does, in a line.
fn summary(s: &Session, command: &str, at: &Json, reason: &str) -> String {
    let label = s.registry.get(command).map(|c| c.label.trim_end_matches('…').to_string()).unwrap_or_else(|| command.to_string());
    let para = p::u64(at, "paragraph").map(|n| format!(" §{n}")).unwrap_or_default();
    let quote = p::str(at, "quote").map(|q| format!(" “{}”", q.chars().take(40).collect::<String>())).unwrap_or_default();
    let r: String = reason.chars().take(120).collect();
    if r.is_empty() { format!("{label}{para}{quote}") } else { format!("{label}{para}{quote}: {r}") }
}

/// `run_command` with `topic`: recorded in the topic's proposal, checked on its draft; the
/// document doesn't change.
pub(crate) fn propose(s: &mut Session, v: &Json, command: &str, params: Json, reason: String, source: String) -> CmdResult {
    let topic = topic_of(s, v)?;
    let option = option_of(v)?.unwrap_or_else(|| "A".into());
    let at = v.get("at").filter(|a| a.is_object()).cloned().ok_or_else(|| bad("a proposed change needs `at` {paragraph, quote}"))?;
    let l = crate::layer(s);
    let pending = l.topic(&topic).and_then(|t| t.edits.as_ref()).is_some_and(|e| e.state == ProposalState::Pending);
    let earlier = if pending { steps_of(&l, &topic, &option) } else { vec![] };
    let mut sc = scratch(crate::agent_doc(s), &l, false);
    replay(&mut sc, &earlier)
        .map_err(|(i, e)| bad(format!("this option is out of date (step {} no longer fits: {e}): discard_proposal and redo it", i + 1)))?;
    // Where, in the draft's terms: its paragraph, the quote and which occurrence, and the text.
    let view = crate::view(&sc);
    let span = crate::commands::place_in(&view, &at)?.ok_or_else(|| bad("`at` needs `paragraph` and/or `quote`"))?;
    let quote = p::str(&at, "quote").filter(|q| !q.is_empty());
    let mut canon = json!({"paragraph": span.para});
    if let Some(q) = quote {
        let occ = (0..500).take_while(|j| view.find(Some(span.para), Some(q), *j).is_ok_and(|x| x.start < span.start)).count();
        canon["quote"] = json!(q);
        canon["occurrence"] = json!(occ);
    }
    for k in ["through", "throughQuote", "caret"] {
        if let Some(x) = at.get(k) {
            canon[k] = x.clone();
        }
    }
    let para_text = view.para(span.para).map(|m| m.text.clone()).unwrap_or_default();
    crate::commands::exec_at(&mut sc, command, params.clone(), &json!({"at": canon, "actor": "agent"}), reason.clone(), source.clone())?;
    let step = Step { summary: summary(s, command, &canon, &reason), command: command.to_string(), params, at: canon, para_text, reason, source };
    let (t2, o2) = (topic.clone(), option.clone());
    let n = crate::update(s, |l| {
        let Some(t) = l.topic_mut(&t2) else { return 0 };
        let e = t.edits.get_or_insert_with(EditProposal::default);
        if e.state != ProposalState::Pending {
            *e = EditProposal::default();
        }
        e.at = crate::now_ms();
        match e.options.iter_mut().find(|o| o.option == o2) {
            Some(o) => {
                o.steps.push(step);
                o.steps.len()
            }
            None => {
                e.options.push(ProposalOption { option: o2.clone(), steps: vec![step], ..Default::default() });
                e.options.sort_by(|a, b| a.option.cmp(&b.option));
                1
            }
        }
    });
    Ok(
        json!({"ok": true, "proposed": true, "topic": topics::number(s, &topic), "option": option, "changes": n, "hint": "the document is unchanged: the person sees the proposal in the topic and applies it there"}),
    )
}

// ---------------------------------------------------------------- the agent's

fn describe_option(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let topic = topic_of(s, v)?;
    let option = option_of(v)?.ok_or_else(|| bad("`option` is required"))?;
    let title = text_p(v, "title", 200)?;
    let raw = opt_text(v, "why", 2_000);
    // Links inside (§12, quote:…) are kept as anchors, shown with current numbers.
    let why = crate::links::store(s, &raw);
    let (t2, o2) = (topic.clone(), option.clone());
    let ok = crate::update(s, |l| {
        let Some(e) = l.topic_mut(&t2).and_then(|t| t.edits.as_mut()).filter(|e| e.state == ProposalState::Pending) else { return false };
        match e.options.iter_mut().find(|o| o.option == o2) {
            Some(o) => {
                o.title = title;
                o.why = why;
                true
            }
            None => false,
        }
    });
    if !ok {
        return Err(bad(format!(
            "topic {} has no pending option {option}: propose with run_command {{topic, option}} first",
            topics::number(s, &topic).unwrap_or(0)
        )));
    }
    Ok(json!({"topic": topics::number(s, &topic), "option": option}))
}

fn discard(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let topic = topic_of(s, v)?;
    let option = option_of(v)?;
    let t2 = topic.clone();
    crate::update(s, |l| {
        if let Some(t) = l.topic_mut(&t2) {
            match &option {
                Some(o) => {
                    if let Some(e) = t.edits.as_mut() {
                        e.options.retain(|x| &x.option != o);
                        if e.options.is_empty() {
                            t.edits = None;
                        }
                    }
                }
                None => t.edits = None,
            }
        }
    });
    Ok(json!({"topic": topics::number(s, &topic), "discarded": option.unwrap_or_else(|| "all".into())}))
}

fn events(s: &mut Session, v: &Json) -> CmdResult {
    let after = p::u64(v, "after").unwrap_or(0);
    let list: Vec<Json> =
        s.ext.get::<crate::State>().map(|st| st.events.iter().filter(|(n, _)| *n > after).map(|(_, e)| e.clone()).collect()).unwrap_or_default();
    Ok(json!(list))
}

/// Something the agent should hear of (the `/events` stream).
pub(crate) fn event(s: &mut Session, mut e: Json) {
    let doc = crate::document_id(s);
    if let Some(st) = s.ext.get_mut::<crate::State>() {
        let n = st.events.last().map_or(1, |(k, _)| k + 1);
        e["seq"] = json!(n);
        e["document"] = json!(doc);
        st.events.push((n, e));
        if st.events.len() > 500 {
            st.events.drain(..100);
        }
    }
}

// ---------------------------------------------------------------- reads

/// The paragraphs (in the agent's view of the accepted document) that pending proposals touch:
/// `(paragraph, topic id, option, summary)`.
pub fn pending_marks(s: &Session) -> Vec<(usize, String, String, String)> {
    let l = crate::layer(s);
    let mut out = vec![];
    if !l.topics.iter().any(|t| t.edits.as_ref().is_some_and(|e| e.state == ProposalState::Pending)) {
        return out;
    }
    let view = crate::view(s);
    for t in &l.topics {
        let Some(e) = t.edits.as_ref().filter(|e| e.state == ProposalState::Pending) else { continue };
        for o in &e.options {
            for st in &o.steps {
                if let Some(at) = locate(&view, &st.at, &st.para_text)
                    && let Some(n) = p::u64(&at, "paragraph")
                {
                    out.push((n as usize, t.id.clone(), o.option.clone(), st.summary.clone()));
                }
            }
        }
    }
    out
}

/// `pending` for a paragraph of a read.
pub fn pending_json(s: &Session, marks: &[(usize, String, String, String)], para: usize) -> Option<Json> {
    let v: Vec<Json> =
        marks.iter().filter(|m| m.0 == para).map(|(_, t, o, sum)| json!({"topic": topics::number(s, t), "option": o, "summary": sum})).collect();
    (!v.is_empty()).then(|| json!(v))
}

/// `read_document {view: "proposal", topic, option?}`: the draft, read the same way.
pub(crate) fn read_proposal(s: &Session, v: &Json, read: fn(&mut Session, &Json) -> CmdResult) -> CmdResult {
    let topic = topic_of(s, v)?;
    let l = crate::layer(s);
    let option = option_of(v)?
        .or_else(|| l.topic(&topic).and_then(|t| t.edits.as_ref()).and_then(|e| e.options.first()).map(|o| o.option.clone()))
        .unwrap_or_else(|| "A".into());
    let doc = draft(s, &topic, &option, false).map_err(bad)?;
    let mut sc = scratch(&doc, &l, false);
    let mut v2 = v.clone();
    if let Some(m) = v2.as_object_mut() {
        m.remove("view");
        m.remove("topic");
        m.remove("option");
        m.remove("document");
    }
    let mut out = read(&mut sc, &v2)?;
    out["view"] = json!("proposal");
    out["topic"] = json!(topics::number(s, &topic));
    out["option"] = json!(option);
    Ok(out)
}

/// A topic's proposal for the agent and the pane.
pub fn proposal_json(s: &Session, t: &crate::layer::Topic) -> Option<Json> {
    let e = t.edits.as_ref()?;
    Some(json!({
        "state": e.state.id(),
        "chosen": if e.chosen.is_empty() { Json::Null } else { json!(e.chosen) },
        "comment": if e.comment.is_empty() { Json::Null } else { json!(e.comment) },
        "options": e.options.iter().map(|o| json!({
            "option": o.option, "title": o.title, "why": crate::links::for_agent(s, &o.why),
            "changes": o.steps.len(),
            "summary": o.steps.iter().map(|x| x.summary.clone()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------- the person's

fn person(v: &Json) -> Result<(), CmdError> {
    if is_agent(v) {
        return Err(bad("only the person applies or rejects a proposal"));
    }
    Ok(())
}

fn set_state(s: &mut Session, topic: &str, f: impl FnOnce(&mut EditProposal)) -> Option<(EditProposal, EditProposal)> {
    let t = topic.to_string();
    crate::update(s, |l| {
        let e = l.topic_mut(&t)?.edits.as_mut()?;
        let before = e.clone();
        f(e);
        Some((before, e.clone()))
    })
}

/// Checks the other pending proposals against the document now: the ones that no longer fit are
/// out of date.
fn recheck(s: &mut Session, except: &str) {
    let l = crate::layer(s);
    for t in l.topics.iter().filter(|t| t.id != except) {
        let Some(e) = t.edits.as_ref().filter(|e| e.state == ProposalState::Pending) else { continue };
        let stale: Vec<String> = e.options.iter().filter(|o| draft(s, &t.id, &o.option, false).is_err()).map(|o| o.option.clone()).collect();
        if !stale.is_empty() {
            set_state(s, &t.id, |e| e.state = ProposalState::Outdated);
            event(
                s,
                json!({"event": "proposal_outdated", "topic": topics::number(s, &t.id), "options": stale, "hint": "the document changed under it: read again and redo the proposal"}),
            );
        }
    }
}

fn apply(s: &mut Session, v: &Json) -> CmdResult {
    person(v)?;
    crate::ensure(s);
    let topic = topic_of(s, v)?;
    let l = crate::layer(s);
    let e = l
        .topic(&topic)
        .and_then(|t| t.edits.clone())
        .filter(|e| e.state == ProposalState::Pending)
        .ok_or_else(|| bad("this topic has no proposal waiting"))?;
    let option = option_of(v)?.or_else(|| e.options.first().map(|o| o.option.clone())).unwrap_or_else(|| "A".into());
    let opt = e.options.iter().find(|o| o.option == option).cloned().ok_or_else(|| bad(format!("no option {option}")))?;
    // First on a copy: if a step no longer fits, nothing is applied.
    if let Err(why) = draft(s, &topic, &option, false) {
        set_state(s, &topic, |e| e.state = ProposalState::Outdated);
        event(s, json!({"event": "proposal_outdated", "topic": topics::number(s, &topic), "options": [option], "why": why}));
        return Err(bad("out of date, ask the agent to redo"));
    }
    // Then on the document, every command joining this one's undo step.
    let reader = s.sel.clone();
    for st in &opt.steps {
        let view = crate::view(s);
        let at = locate(&view, &st.at, &st.para_text).ok_or_else(|| bad("out of date, ask the agent to redo"))?;
        s.join_next_undo();
        crate::commands::exec_at(s, &st.command, st.params.clone(), &json!({"at": at, "actor": "agent"}), st.reason.clone(), st.source.clone())?;
    }
    s.sel = wordcraft_engine::Selection { anchor: s.doc.clamp(&reader.anchor), focus: s.doc.clamp(&reader.focus) };
    let title = if opt.title.is_empty() { format!("{} change(s)", opt.steps.len()) } else { opt.title.clone() };
    let Some((before, after)) = set_state(s, &topic, |e| {
        e.state = ProposalState::Applied;
        e.chosen = option.clone();
    }) else {
        return Err(bad("this topic has no proposal waiting"));
    };
    // The decision, in the topic and the history.
    let who = s.author.clone();
    let (t2, text) = (topic.clone(), format!("chose {option}: {title}"));
    let note = crate::update(s, |l| {
        let id = l.fresh_id("n");
        let n = Note {
            id,
            kind: "decision".into(),
            title: text.clone(),
            text: text.clone(),
            author: Actor::Human,
            at: crate::now_ms(),
            topic: t2.clone(),
            ..Default::default()
        };
        l.notes.push(n.clone());
        l.log(LogEntry {
            at: crate::now_ms(),
            actor: Actor::Human,
            kind: "decision".into(),
            text: format!("Applied the proposal: {text}"),
            command: "agent.applyProposal".into(),
            who,
            ..Default::default()
        });
        n
    });
    undo::push(s, Op::Proposal { topic: topic.clone(), before: Box::new(before), after: Box::new(after), question: None, note: Some(note) });
    event(s, json!({"event": "proposal_applied", "topic": topics::number(s, &topic), "option": option, "title": title}));
    recheck(s, &topic);
    Ok(json!({"topic": topics::number(s, &topic), "applied": option, "changes": opt.steps.len()}))
}

fn reject(s: &mut Session, v: &Json) -> CmdResult {
    person(v)?;
    crate::ensure(s);
    let topic = topic_of(s, v)?;
    let comment = opt_text(v, "comment", 4_000);
    let Some((before, after)) = set_state(s, &topic, |e| {
        e.state = ProposalState::Rejected;
        e.comment = comment.clone();
    }) else {
        return Err(bad("this topic has no proposal"));
    };
    // The comment reaches the agent as a follow-up in the topic.
    let question = (!comment.trim().is_empty()).then(|| {
        let l = crate::layer(s);
        let anchor = l.topic(&topic).and_then(|t| t.places.first()).map(|p| p.anchor.clone()).unwrap_or_default();
        let (t2, asked_by, text) = (topic.clone(), s.author.clone(), format!("I rejected your proposal: {}", comment.trim()));
        crate::update(s, |l| {
            let id = l.fresh_id("q");
            l.questions.push(Question {
                id: id.clone(),
                anchor,
                text,
                status: QuestionStatus::Pending,
                asked_at: crate::now_ms(),
                asked_by,
                topic: t2,
                ..Default::default()
            });
            id
        })
    });
    let who = s.author.clone();
    crate::update(s, |l| {
        l.log(LogEntry {
            at: crate::now_ms(),
            actor: Actor::Human,
            kind: "decision".into(),
            text: format!("Rejected the proposal{}", if comment.is_empty() { String::new() } else { format!(": {comment}") }),
            command: "agent.rejectProposal".into(),
            who,
            ..Default::default()
        })
    });
    undo::push(s, Op::Proposal { topic: topic.clone(), before: Box::new(before), after: Box::new(after), question: question.clone(), note: None });
    event(s, json!({"event": "proposal_rejected", "topic": topics::number(s, &topic), "comment": comment, "question": question}));
    Ok(json!({"topic": topics::number(s, &topic), "rejected": true, "question": question}))
}

fn preview(s: &mut Session, v: &Json) -> CmdResult {
    person(v)?;
    let want = match v.get("topic") {
        None | Some(Json::Null) => None,
        Some(_) => {
            let topic = topic_of(s, v)?;
            let option = option_of(v)?.unwrap_or_else(|| "A".into());
            Some((topic, option))
        }
    };
    crate::set_proposal_preview(s, want.clone()).map_err(bad)?;
    Ok(json!({"preview": want.map(|(t, o)| json!({"topic": t, "option": o}))}))
}
