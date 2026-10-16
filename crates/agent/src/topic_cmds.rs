//! Topic commands (`~/aitocraft/TOPICS.md` §1, §4).
//!
//! The person's (changing commands, each an undo step: `crate::undo`): `agent.ask`,
//! `agent.setStatus`, `agent.confirmAgreed`, `agent.dismissProposal`, `agent.pinNote`,
//! `agent.rejectPlace`, `agent.addPlace`. The agent's (never on the person's undo stack):
//! `agent.topics`, `agent.topic`, `agent.addPlaces`, `agent.widenPlace`, `agent.topicStatus`,
//! `agent.proposeAgreed`. Only a person sets Agreed.

use serde_json::{Value as Json, json};
use wordcraft_doc::{Pos, StoryRef};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::commands::{bad, doc_check, gate, is_agent, masked, opt_text, text_p};
use crate::layer::{Actor, LogEntry, Proposal, Question, QuestionStatus, StatusKind, TopicStatus};
use crate::topics;
use crate::undo::{self, Op};

const LOC: &str = "Review › Agent";

pub fn specs() -> Vec<CommandSpec> {
    let c = CommandSpec::new;
    vec![
        // the person's (undoable)
        c("agent.ask", "Ask the Agent", LOC, ask).params(
            r#"{"text": string, "paragraph"?: number, "quote"?: string, "topic"?: id|number (a follow-up), "newTopic"?: bool, "refs"?: [{"n": number, "paragraph"?: number, "quote"?: string, "occurrence"?: number}] (what ⟦n⟧ in the text points at: a place each), "spoken"?: bool, "transcribing"?: bool (spoken, text still coming: not with the agent until agent.spoken)} — on the selection; a selection that already has a topic adds to it"#,
        ),
        c("agent.spoken", "Spoken Question Transcribed", LOC, spoken)
            .params(r#"{"id": string, "text"?: string} — the text is ready: the question goes to the agent; {"id", "error"} — the transcription failed; {"id"} — transcribing again"#)
            .pure(),
        c("agent.removeQuestion", "Dismiss Spoken Question", LOC, remove_question)
            .params(r#"{"id": string} — a spoken question still being transcribed, or whose transcription failed (its new topic goes with it)"#),
        c("agent.setStatus", "Set Topic Status", LOC, set_status).params(r#"{"topic": id|number, "status": "open"|"revise"|"agreed"|"info", "text"?: string}"#),
        c("agent.confirmAgreed", "Confirm Agreed", LOC, confirm_agreed).params(r#"{"topic": id|number} — the agent's proposal, accepted"#),
        c("agent.dismissProposal", "Dismiss Proposal", LOC, dismiss_proposal).params(r#"{"topic": id|number}"#),
        c("agent.pinNote", "Pin Note", LOC, pin_note).params(r#"{"id": string, "on"?: bool}"#),
        c("agent.rejectPlace", "Reject Place", LOC, reject_place).params(r#"{"place": id|label} — an agent's place, taken off the topic"#),
        c("agent.addPlace", "Add Place to Topic", LOC, add_place).params(r#"{"topic": id|number} — the selection becomes another place of the topic"#),
        // the agent's
        c("agent.topics", "Topics", LOC, topics_cmd).params(r#"{"status"?: "open"|"revise"|"agreed"|"info"} → topics with places, status, note titles, unread flags"#).pure(),
        c("agent.topic", "Topic", LOC, topic_cmd).params(r#"{"topic": id|number} → everything, discussion included"#).pure(),
        c("agent.addPlaces", "Add Places", LOC, add_places)
            .params(r#"{"topic": id|number, "places": [{"paragraph": number, "quote"?: string, "through"?: number (last paragraph), "reason": string}]}"#)
            .pure(),
        c("agent.widenPlace", "Widen Place", LOC, widen_place)
            .params(r#"{"place": label|id, "to": {"paragraph": number, "quote"?: string, "through"?: number}, "reason": string}"#)
            .pure(),
        c("agent.topicStatus", "Set Status (Agent)", LOC, topic_status).params(r#"{"topic": id|number, "status": "open"|"revise"|"info", "text": string} — Agreed is the person's"#).pure(),
        c("agent.proposeAgreed", "Propose Agreed", LOC, propose_agreed).params(r#"{"topic": id|number, "reasons": string}"#).pure(),
        c("agent.saveForSending", "Save a Copy to Send", LOC, save_for_sending)
            .params(r#"{"path": string, "includeDiscussion"?: bool} — the document with places, statuses and notes; the discussion only when asked"#)
            .pure(),
    ]
}

fn topic_p(s: &Session, v: &Json) -> Result<String, CmdError> {
    let raw = match v.get("topic") {
        Some(Json::String(t)) => t.clone(),
        Some(Json::Number(n)) => n.to_string(),
        _ => return Err(bad("`topic` (id or number) is required")),
    };
    topics::find(s, &raw).ok_or_else(|| bad(format!("no topic `{raw}`")))
}

fn status_name(s: &Session, topic: &str) -> String {
    format!("topic {}", topics::number(s, topic).unwrap_or(0))
}

fn log(s: &mut Session, actor: Actor, kind: &str, text: String, command: &str, source: String) {
    let who = if actor == Actor::Agent { "agent".to_string() } else { s.author.clone() };
    crate::update(s, |l| {
        l.log(LogEntry { at: crate::now_ms(), actor, kind: kind.into(), text, command: command.into(), source, who, ..Default::default() })
    });
}

/// Where a reader's command applies: the given paragraph/quote, else the selection (a caret: its
/// paragraph). Positions in the document on screen.
pub(crate) fn reader_range(s: &Session, v: &Json) -> Result<(Pos, Pos), CmdError> {
    if p::u64(v, "paragraph").is_some() || p::str(v, "quote").is_some() {
        let plain = crate::mask::View::new(&s.doc, &Default::default(), &[]);
        let para = p::u64(v, "paragraph").map(|n| n.min(usize::MAX as u64) as usize);
        let occ = p::u64(v, "occurrence").unwrap_or(0).min(10_000) as usize;
        let sp = plain.find(para, p::str(v, "quote"), occ).map_err(bad)?;
        return Ok((sp.start_pos(), sp.end_pos()));
    }
    let (a, b) = s.sel.ordered();
    if a.story != StoryRef::Body || b.story != StoryRef::Body {
        return Err(bad("select text in the body of the document first"));
    }
    if a == b {
        let len = crate::anchor::para_text(&s.doc, &a.path).map_or(0, str::len);
        return Ok((Pos { off: 0, ..a.clone() }, Pos { off: len, ..a }));
    }
    Ok((a, b))
}

/// The passages pointed at while speaking (`refs`), in the document on screen.
pub(crate) fn ref_ranges(s: &Session, v: &Json) -> Result<Vec<(u32, Pos, Pos)>, CmdError> {
    let mut out: Vec<(u32, Pos, Pos)> = vec![];
    for r in v.get("refs").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(50) {
        let n = r
            .get("n")
            .and_then(Json::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| bad("each ref needs `n` (1, 2, …)"))?;
        if p::u64(r, "paragraph").is_none() && p::str(r, "quote").is_none() {
            return Err(bad("each ref needs `paragraph`, `quote` or both"));
        }
        if out.iter().any(|(m, ..)| *m == n) {
            continue;
        }
        let (a, b) = reader_range(s, r)?;
        out.push((n, a, b));
    }
    Ok(out)
}

/// Asked at the caret (no selection, no passage named).
pub(crate) fn at_caret(s: &Session, v: &Json) -> bool {
    p::u64(v, "paragraph").is_none() && p::str(v, "quote").is_none() && s.sel.is_collapsed()
}

// ---------------------------------------------------------------- the person's

/// Ask: on a selection that has a topic, a follow-up in it; else (or with `newTopic`) a new topic
/// with the selection as its place.
pub fn ask(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("questions are the reader's"));
    }
    // On the client's copy: the passage traced back to the template (the observer swapped it in).
    let traced = crate::take_preview_ask();
    if traced.is_none() && crate::render_state(s).is_some() {
        return Err(bad("this passage of the client's copy can't be traced to the template: switch Show: Template to ask"));
    }
    // Spoken: put on the passages now, the text follows (agent.spoken).
    let transcribing = p::bool(v, "transcribing").unwrap_or(false);
    let text = if transcribing { opt_text(v, "text", 20_000) } else { text_p(v, "text", 20_000)? };
    let spoken = transcribing || p::bool(v, "spoken").unwrap_or(false);
    crate::ensure(s);
    let explicit = match v.get("topic") {
        Some(Json::Null) | None => None,
        Some(_) => Some(topic_p(s, v)?),
    };
    let new_topic = p::bool(v, "newTopic").unwrap_or(false);
    let (a, b, caret, refs) = match &traced {
        Some(t) => (t.main.0.clone(), t.main.1.clone(), t.caret, t.refs.clone()),
        None => {
            let (a, b) = reader_range(s, v)?;
            (a, b, at_caret(s, v), ref_ranges(s, v)?)
        }
    };
    let touching = |s: &Session, a: &Pos, b: &Pos| topics::touching(s, a, b).first().map(|t| t.id.clone());
    let existing = explicit.or_else(|| {
        if new_topic {
            return None;
        }
        touching(s, &a, &b).or_else(|| refs.first().and_then(|(_, x, y)| touching(s, x, y)))
    });
    let (topic, created) = match existing {
        Some(t) => (t, false),
        None => {
            let t = crate::update(s, |l| l.fresh_id("t"));
            let tid = t.clone();
            crate::update(s, |l| {
                l.topics.push(crate::layer::Topic { id: tid.clone(), created_by: Actor::Human, created_at: crate::now_ms(), ..Default::default() });
            });
            (t, true)
        }
    };
    // The places: the passage asked about (a new topic's; not a bare caret when something was
    // pointed at), and one per passage pointed at, unless a place of the topic already covers it.
    // Offsets without markers first, so bookmarks in one paragraph don't shift each other.
    let mut wanted: Vec<(Option<u32>, Pos, Pos)> = vec![];
    if created && (refs.is_empty() || !caret) {
        wanted.push((None, a.clone(), b.clone()));
    }
    wanted.extend(refs.iter().map(|(n, x, y)| (Some(*n), x.clone(), y.clone())));
    let plain: Vec<(Option<u32>, Pos, Pos)> =
        wanted.iter().map(|(n, x, y)| (*n, crate::text_offset(&s.doc, x), crate::text_offset(&s.doc, y))).collect();
    let mut qrefs: Vec<crate::layer::QuestionRef> = vec![];
    for (n, pa, pb) in plain {
        let x = s.doc.clamp(&crate::from_text_offset(&s.doc, &pa, true));
        let y = s.doc.clamp(&crate::from_text_offset(&s.doc, &pb, false));
        let covered = n.and_then(|_| {
            topics::resolve(s)
                .iter()
                .find(|t| t.id == topic)
                .and_then(|t| t.places.iter().find(|p| p.ranges().any(|(f, e)| *f <= y && x <= *e)).map(|p| p.id.clone()))
        });
        let place = match covered {
            Some(p) => p,
            None => topics::add_place(s, &topic, &x, &y, Actor::Human, "").map_err(CmdError::Failed)?,
        };
        if let Some(n) = n {
            qrefs.push(crate::layer::QuestionRef { n, place });
        }
    }
    let l = crate::layer(s);
    let anchor = l.topic(&topic).and_then(|t| t.places.first()).map(|p| p.anchor.clone()).unwrap_or_default();
    let asked_by = s.author.clone();
    let id = crate::update(s, |l| {
        let id = l.fresh_id("q");
        l.questions.push(Question {
            id: id.clone(),
            anchor,
            text: text.clone(),
            status: if transcribing { QuestionStatus::Transcribing } else { QuestionStatus::Pending },
            asked_at: crate::now_ms(),
            asked_by,
            topic: topic.clone(),
            refs: qrefs,
            spoken,
            ..Default::default()
        });
        id
    });
    if !transcribing {
        let name = status_name(s, &topic);
        log(s, Actor::Human, "question", format!("Asked on {name}: “{}”", text.chars().take(200).collect::<String>()), "agent.ask", id.clone());
    }
    undo::push(s, Op::Ask { question: id.clone() });
    // Where it went, when asked on the client's copy.
    let on_template = match traced {
        Some(t) if t.on_loop => json!("loop"),
        Some(_) => json!("paragraph"),
        None => Json::Null,
    };
    Ok(json!({"id": id, "topic": topic, "number": topics::number(s, &topic), "newTopic": created, "onTemplate": on_template}))
}

/// A spoken question's transcription: done (the question goes to the agent), failed, or again.
fn spoken(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("questions are the reader's"));
    }
    let id = text_p(v, "id", 100)?;
    let text: Option<String> = p::str(v, "text").map(str::trim).filter(|t| !t.is_empty()).map(|t| t.chars().take(20_000).collect());
    let error: Option<String> = p::str(v, "error").map(|e| e.chars().take(2_000).collect());
    crate::ensure(s);
    let r = crate::update(s, |l| {
        let Some(q) = l.questions.iter_mut().find(|q| q.id == id) else { return Err(format!("no question `{id}`")) };
        if !q.status.is_draft() {
            return Err(format!("question `{id}` is already with the agent"));
        }
        match (&text, error) {
            (Some(t), _) => {
                q.text = t.clone();
                q.error.clear();
                q.status = QuestionStatus::Pending;
                q.asked_at = crate::now_ms();
            }
            (None, Some(e)) => {
                q.error = if e.trim().is_empty() { "The transcription failed".into() } else { e };
                q.status = QuestionStatus::Failed;
            }
            (None, None) => {
                q.error.clear();
                q.status = QuestionStatus::Transcribing;
            }
        }
        Ok((format!("{:?}", q.status).to_lowercase(), q.topic.clone()))
    });
    let (status, topic) = r.map_err(bad)?;
    if let Some(t) = text {
        let name = status_name(s, &topic);
        log(s, Actor::Human, "question", format!("Asked on {name} (spoken): “{}”", t.chars().take(200).collect::<String>()), "agent.ask", id.clone());
    }
    Ok(json!({"id": id, "status": status}))
}

/// Drops a spoken question that never became text; a topic it alone made goes with it.
fn remove_question(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("questions are the reader's"));
    }
    let id = text_p(v, "id", 100)?;
    crate::ensure(s);
    let l = crate::layer(s);
    let q = l.questions.iter().find(|q| q.id == id).cloned().ok_or_else(|| bad(format!("no question `{id}`")))?;
    if !q.status.is_draft() {
        return Err(bad(format!("question `{id}` is with the agent already")));
    }
    let alone = l.topic(&q.topic).is_some_and(|t| t.created_by == Actor::Human)
        && !l.questions.iter().any(|x| x.topic == q.topic && x.id != id && !x.hidden)
        && !l.notes.iter().any(|n| n.topic == q.topic);
    let places: Vec<String> =
        if alone { l.topic(&q.topic).map(|t| t.places.iter().map(|p| p.id.clone()).collect()).unwrap_or_default() } else { vec![] };
    for p in &places {
        let m = crate::places::scan(&s.doc);
        for o in m.get(&crate::layer::bookmark_name(p)).cloned().unwrap_or_default().iter().rev() {
            crate::places::remove(&mut s.doc, o);
        }
    }
    let topic = q.topic.clone();
    crate::update(s, |l| {
        l.questions.retain(|x| x.id != id);
        if alone {
            l.topics.retain(|t| t.id != topic);
        }
    });
    s.touch();
    Ok(json!({"id": id, "topicRemoved": alone}))
}

fn apply_status(s: &mut Session, topic: &str, new: TopicStatus, proposal: Option<Proposal>, undoable: bool) -> CmdResult {
    let Some(t) = crate::layer(s).topic(topic).cloned() else { return Err(bad(format!("no topic `{topic}`"))) };
    let before = (t.status.clone(), t.proposal.clone());
    let after = (new.clone(), proposal.clone());
    let tid = topic.to_string();
    crate::update(s, |l| {
        if let Some(x) = l.topic_mut(&tid) {
            x.status = after.0.clone();
            x.proposal = after.1.clone();
        }
    });
    let shown = if new.text.is_empty() { String::new() } else { format!(": {}", crate::links::plain(s, &new.text)) };
    let name = status_name(s, topic);
    log(s, new.by, "status", format!("{name} → {}{shown}", new.kind.label()), "agent.setStatus", String::new());
    if undoable {
        undo::push(s, Op::Status { topic: tid.clone(), before, after });
    }
    Ok(json!({"topic": tid, "status": new.kind.id()}))
}

fn person_status(s: &mut Session, kind: StatusKind, text: String) -> TopicStatus {
    TopicStatus { kind, text, by: Actor::Human, who: s.author.clone(), at: crate::now_ms(), set: true }
}

fn set_status(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("use set_status (Open, Revise, For information) or propose_agreed"));
    }
    let topic = topic_p(s, v)?;
    let kind = p::str(v, "status").and_then(StatusKind::parse).ok_or_else(|| bad("`status` is open, revise, agreed or info"))?;
    let raw = opt_text(v, "text", 4_000);
    let text = crate::links::store(s, &raw);
    let st = person_status(s, kind, text);
    // A decision supersedes the agent's proposal.
    let keep = crate::layer(s).topic(&topic).and_then(|t| t.proposal.clone()).filter(|_| kind != StatusKind::Agreed);
    apply_status(s, &topic, st, keep, true)
}

fn confirm_agreed(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("only a person sets Agreed"));
    }
    let topic = topic_p(s, v)?;
    let reasons = crate::layer(s).topic(&topic).and_then(|t| t.proposal.clone()).map(|p| p.reasons).unwrap_or_default();
    let st = person_status(s, StatusKind::Agreed, reasons);
    apply_status(s, &topic, st, None, true)
}

fn dismiss_proposal(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("the person dismisses proposals"));
    }
    let topic = topic_p(s, v)?;
    let st = crate::layer(s).topic(&topic).map(|t| t.status.clone()).unwrap_or_default();
    apply_status(s, &topic, st, None, true)
}

fn pin_note(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("pinning is the reader's"));
    }
    let id = text_p(v, "id", 100)?;
    let Some(before) = crate::layer(s).notes.iter().find(|n| n.id == id).map(|n| n.pinned) else { return Err(bad(format!("no note `{id}`"))) };
    let after = p::bool(v, "on").unwrap_or(!before);
    let nid = id.clone();
    crate::update(s, |l| {
        if let Some(n) = l.notes.iter_mut().find(|n| n.id == nid) {
            n.pinned = after;
        }
    });
    undo::push(s, Op::PinNote { note: id.clone(), before, after });
    Ok(json!({"id": id, "pinned": after}))
}

fn reject_place(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("rejecting a place is the reader's"));
    }
    let key = text_p(v, "place", 100)?;
    let id = topics::find_place(s, &key).ok_or_else(|| bad(format!("no place `{key}`")))?;
    let pid = id.clone();
    crate::update(s, |l| {
        for t in &mut l.topics {
            if let Some(p) = t.places.iter_mut().find(|p| p.id == pid) {
                p.rejected = true;
            }
        }
    });
    // Its bookmark goes now (and comes back with an undo).
    let m = crate::places::scan(&s.doc);
    for o in m.get(&crate::layer::bookmark_name(&id)).cloned().unwrap_or_default().iter().rev() {
        crate::places::remove(&mut s.doc, o);
    }
    undo::push(s, Op::RejectPlace { place: id.clone() });
    Ok(json!({"place": id, "rejected": true}))
}

fn add_place(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("use add_places"));
    }
    if crate::render_state(s).is_some() {
        return Err(bad("switch Show: Template to mark places"));
    }
    let topic = topic_p(s, v)?;
    let (a, b) = reader_range(s, v)?;
    let id = topics::add_place(s, &topic, &a, &b, Actor::Human, "").map_err(CmdError::Failed)?;
    undo::push(s, Op::AddPlace);
    Ok(json!({"place": id, "topic": topic}))
}

// ---------------------------------------------------------------- the agent's

/// A place as the agent sees it (masked text, current numbers).
fn place_json(s: &Session, view: &crate::mask::View, p: &topics::RPlace) -> Json {
    let para = p.from.as_ref().and_then(|f| topics::para_index(s, f));
    let quote = match (&p.removed, para, &p.from, &p.to) {
        (Some(r), ..) => view.scrub(r),
        (None, Some(i), Some(a), Some(b)) => {
            let end = if b.path == a.path { b.off } else { usize::MAX };
            view.para(i).map(|m| m.masked_slice(a.off, end.min(m.segs.last().map_or(0, |x| x.r.1)))).unwrap_or("").to_string()
        }
        _ => String::new(),
    };
    let mut o = json!({"place": p.label, "id": p.id, "paragraph": para, "quote": quote});
    if let (Some(a), Some(b)) = (&p.from, &p.to)
        && b.path != a.path
    {
        o["throughParagraph"] = json!(topics::para_index(s, b));
    }
    if p.removed.is_some() {
        o["removed"] = json!(true);
    }
    if !p.reason.is_empty() {
        o["reason"] = json!(p.reason);
    }
    if p.added_by == Actor::Agent {
        o["addedBy"] = json!("agent");
    }
    o
}

/// A topic for the agent; `full` adds the notes' text and the discussion.
pub fn topic_json(s: &Session, view: &crate::mask::View, t: &topics::RTopic, full: bool) -> Json {
    let l = crate::layer(s);
    let Some(topic) = l.topic(&t.id) else { return Json::Null };
    let notes: Vec<Json> = l
        .notes
        .iter()
        .filter(|n| n.topic == t.id)
        .map(|n| {
            let mut o = json!({"id": n.id, "title": n.title, "kind": n.kind, "by": if n.author == Actor::Agent { "agent" } else { "person" }});
            if full {
                o["text"] = json!(crate::links::for_agent(s, &n.text));
            }
            o
        })
        .collect();
    let qs: Vec<&Question> = l.questions.iter().filter(|q| q.topic == t.id && !q.hidden).collect();
    let unread = qs.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
    let mut o = json!({
        "topic": t.number,
        "id": t.id,
        "status": topic.status.kind.id(),
        "statusText": crate::links::for_agent(s, &topic.status.text),
        "statusBy": if topic.status.by == Actor::Agent { "agent" } else { "person" },
        "places": t.places.iter().map(|p| place_json(s, view, p)).collect::<Vec<_>>(),
        "notes": notes,
        "openQuestions": qs.iter().filter(|q| q.status.is_open()).count(),
        "unreadAnswers": unread,
    });
    if let Some(p) = &topic.proposal {
        o["proposal"] = json!({"status": "agreed", "reasons": crate::links::for_agent(s, &p.reasons)});
    }
    if full {
        let mut d: Vec<&Question> = qs.clone();
        d.sort_by_key(|q| q.asked_at);
        o["discussion"] = Json::Array(
            d.iter()
                .map(|q| {
                    json!({
                        "question": q.id,
                        "asked": q.text,
                        "status": format!("{:?}", q.status).to_lowercase(),
                        "answer": q.answer.as_ref().map(|a| crate::links::for_agent(s, &a.text)),
                    })
                })
                .collect(),
        );
    }
    o
}

fn topics_cmd(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let want = p::str(v, "status").and_then(StatusKind::parse);
    // Places are read in the template (the client's copy on screen is not the agent's).
    let swapped = crate::template_in(s);
    let view = crate::view(s);
    let l = crate::layer(s);
    let all = topics::resolve(s);
    let list: Vec<Json> = all
        .iter()
        .filter(|t| want.is_none_or(|w| l.topic(&t.id).is_some_and(|x| x.status.kind == w)))
        .map(|t| topic_json(s, &view, t, false))
        .collect();
    let out = json!({"signOff": sign_off(s), "topics": list});
    crate::template_out(s, swapped);
    masked(&view, v, out)
}

fn topic_cmd(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let id = topic_p(s, v)?;
    let swapped = crate::template_in(s);
    let view = crate::view(s);
    let all = topics::resolve(s);
    let out = all.iter().find(|t| t.id == id).map(|t| topic_json(s, &view, t, true));
    crate::template_out(s, swapped);
    let out = out.ok_or_else(|| bad("that topic isn't shown (undone)"))?;
    masked(&view, v, out)
}

/// "Agreed 10 of 15 · 2 revise · 3 open".
pub fn sign_off(s: &Session) -> Json {
    let l = crate::layer(s);
    let all = topics::resolve(s);
    let count = |k: StatusKind| all.iter().filter(|t| l.topic(&t.id).is_some_and(|x| x.status.kind == k)).count();
    let (agreed, revise, open, info) = (count(StatusKind::Agreed), count(StatusKind::Revise), count(StatusKind::Open), count(StatusKind::Info));
    json!({"topics": all.len(), "agreed": agreed, "revise": revise, "open": open, "info": info,
        "line": format!("Agreed {agreed} of {} · {revise} revise · {open} open", agreed + revise + open)})
}

/// A place spec `{paragraph, quote?, through?}` in the agent's view → a range of the agent's document.
fn agent_range(view: &crate::mask::View, v: &Json) -> Result<(Pos, Pos), CmdError> {
    let para = p::u64(v, "paragraph").map(|n| n.min(usize::MAX as u64) as usize);
    let sp = view.find(para, p::str(v, "quote"), p::u64(v, "occurrence").unwrap_or(0).min(10_000) as usize).map_err(bad)?;
    let mut b = sp.end_pos();
    if let Some(t) = p::u64(v, "through").map(|n| n.min(usize::MAX as u64) as usize) {
        let m = view.para(t).ok_or_else(|| bad(format!("no paragraph {t}")))?;
        if t > sp.para {
            b = Pos::new(StoryRef::Body, m.path.clone(), m.segs.last().map_or(0, |x| x.r.1));
        }
    }
    Ok((sp.start_pos(), b))
}

fn add_places(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let topic = topic_p(s, v)?;
    let list = v.get("places").and_then(Json::as_array).cloned().ok_or_else(|| bad("`places` is a list of {paragraph, quote?, reason}"))?;
    if list.is_empty() || list.len() > 50 {
        return Err(bad("give 1 to 50 places"));
    }
    let swapped = crate::template_in(s);
    let mut added = Vec::new();
    let mut err = None;
    for spec in &list {
        let view = crate::view(s);
        let r = agent_range(&view, spec).and_then(|(a, b)| {
            let reason = opt_text(spec, "reason", 500);
            topics::add_place(s, &topic, &a, &b, Actor::Agent, &reason).map_err(CmdError::Failed)
        });
        match r {
            Ok(id) => added.push(id),
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    s.touch();
    crate::template_out(s, swapped);
    if let Some(e) = err
        && added.is_empty()
    {
        return Err(e);
    }
    let labels: Vec<Json> = added.iter().map(|p| json!({"id": p, "place": topics::label(s, p)})).collect();
    let name = status_name(s, &topic);
    log(s, Actor::Agent, "change", format!("Added {} place(s) to {name}", added.len()), "agent.addPlaces", String::new());
    Ok(json!({"topic": topics::number(s, &topic), "added": labels}))
}

fn widen_place(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let key = text_p(v, "place", 100)?;
    let id = topics::find_place(s, &key).ok_or_else(|| bad(format!("no place `{key}`")))?;
    let to = v.get("to").filter(|t| t.is_object()).ok_or_else(|| bad("`to` is {paragraph, quote?, through?}"))?;
    let reason = text_p(v, "reason", 500)?;
    let swapped = crate::template_in(s);
    let view = crate::view(s);
    let r = agent_range(&view, to).and_then(|(a, b)| topics::move_place(s, &id, &a, &b).map_err(CmdError::Failed));
    crate::template_out(s, swapped);
    r?;
    let pid = id.clone();
    crate::update(s, |l| {
        for t in &mut l.topics {
            if let Some(p) = t.places.iter_mut().find(|p| p.id == pid) {
                p.reason = reason.clone();
                p.restore = true;
            }
        }
    });
    Ok(json!({"place": topics::label(s, &id), "widened": true}))
}

fn topic_status(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let topic = topic_p(s, v)?;
    let kind = p::str(v, "status").and_then(StatusKind::parse).ok_or_else(|| bad("`status` is open, revise or info"))?;
    let agent = is_agent(v);
    if agent && kind == StatusKind::Agreed {
        return Err(bad("only a person sets Agreed: use propose_agreed with your reasons"));
    }
    let raw = opt_text(v, "text", 4_000);
    let text = crate::links::store(s, &raw);
    let (by, who) = if agent { (Actor::Agent, "agent".to_string()) } else { (Actor::Human, s.author.clone()) };
    let st = TopicStatus { kind, text, by, who, at: crate::now_ms(), set: true };
    let keep = crate::layer(s).topic(&topic).and_then(|t| t.proposal.clone());
    apply_status(s, &topic, st, keep, false)
}

fn propose_agreed(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let topic = topic_p(s, v)?;
    let raw = text_p(v, "reasons", 4_000)?;
    let reasons = crate::links::store(s, &raw);
    let tid = topic.clone();
    let r = reasons.clone();
    crate::update(s, |l| {
        if let Some(t) = l.topic_mut(&tid) {
            t.proposal = Some(Proposal { reasons: r, at: crate::now_ms() });
        }
    });
    let name = status_name(s, &topic);
    log(s, Actor::Agent, "status", format!("Agent suggests: {name} Agreed"), "agent.proposeAgreed", String::new());
    Ok(json!({"topic": topics::number(s, &topic), "proposed": "agreed", "note": "the person confirms it"}))
}

/// The layer as it is sent: places, statuses and notes; the discussion (questions, answers, and
/// their lines in the history) only with `include`.
pub fn for_sending(l: &crate::layer::Layer, include: bool) -> crate::layer::Layer {
    let mut l = l.clone();
    if !include {
        l.questions.clear();
        l.log.retain(|e| e.kind != "question" && e.command != "agent.answer");
    }
    l
}

#[cfg(not(target_arch = "wasm32"))]
fn save_for_sending(s: &mut Session, v: &Json) -> CmdResult {
    if is_agent(v) {
        return Err(bad("sending is the reader's"));
    }
    let path = text_p(v, "path", 4_000)?;
    let include = p::bool(v, "includeDiscussion").unwrap_or(false);
    let l = for_sending(&crate::layer(s), include);
    let mut doc = crate::agent_doc(s).clone();
    crate::layer::write_into(&mut doc, &l);
    doc.passthrough.remove(undo::MARK);
    doc.settings.protection = None;
    wordcraft_engine::io::save_path(std::path::Path::new(&path), &doc).map_err(CmdError::Failed)?;
    Ok(json!({"path": path, "discussion": include}))
}

#[cfg(target_arch = "wasm32")]
fn save_for_sending(_s: &mut Session, _v: &Json) -> CmdResult {
    Err(bad("not available on the web"))
}
