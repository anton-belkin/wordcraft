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
            r#"{"text": string, "paragraph"?: number, "quote"?: string, "topic"?: id|number (a follow-up), "newTopic"?: bool} — on the selection; a selection that already has a topic adds to it"#,
        ),
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
    let text = text_p(v, "text", 20_000)?;
    crate::ensure(s);
    let explicit = match v.get("topic") {
        Some(Json::Null) | None => None,
        Some(_) => Some(topic_p(s, v)?),
    };
    let new_topic = p::bool(v, "newTopic").unwrap_or(false);
    let (a, b) = match &traced {
        Some((a, b, _)) => (a.clone(), b.clone()),
        None => reader_range(s, v)?,
    };
    let existing = explicit.or_else(|| if new_topic { None } else { topics::touching(s, &a, &b).first().map(|t| t.id.clone()) });
    let (topic, created) = match existing {
        Some(t) => (t, false),
        None => {
            let t = crate::update(s, |l| l.fresh_id("t"));
            let tid = t.clone();
            crate::update(s, |l| {
                l.topics.push(crate::layer::Topic { id: tid.clone(), created_by: Actor::Human, created_at: crate::now_ms(), ..Default::default() });
            });
            topics::add_place(s, &t, &a, &b, Actor::Human, "").map_err(CmdError::Failed)?;
            (t, true)
        }
    };
    let l = crate::layer(s);
    let anchor = l.topic(&topic).and_then(|t| t.places.first()).map(|p| p.anchor.clone()).unwrap_or_default();
    let asked_by = s.author.clone();
    let id = crate::update(s, |l| {
        let id = l.fresh_id("q");
        l.questions.push(Question {
            id: id.clone(),
            anchor,
            text: text.clone(),
            status: QuestionStatus::Pending,
            asked_at: crate::now_ms(),
            asked_by,
            topic: topic.clone(),
            ..Default::default()
        });
        id
    });
    let name = status_name(s, &topic);
    log(s, Actor::Human, "question", format!("Asked on {name}: “{}”", text.chars().take(200).collect::<String>()), "agent.ask", id.clone());
    undo::push(s, Op::Ask { question: id.clone() });
    // Where it went, when asked on the client's copy.
    let on_template = match traced {
        Some((_, _, true)) => json!("loop"),
        Some(_) => json!("paragraph"),
        None => Json::Null,
    };
    Ok(json!({"id": id, "topic": topic, "number": topics::number(s, &topic), "newTopic": created, "onTemplate": on_template}))
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
        "openQuestions": qs.iter().filter(|q| q.status != QuestionStatus::Answered).count(),
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
