//! The file's own context: what the document is, for whom, house rules, background, facts the
//! agent gathered. Not tied to a place; kept in the file's agent layer and kept when the document
//! is sent (the discussion isn't). The agent has a short summary of it with `get_context` and
//! every question, and the whole of it with `get_file_context`. The person's changes are undo
//! steps; the agent's are not (like its notes).

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::commands::{bad, doc_check, gate, is_agent, masked, opt_text, text_p};
use crate::layer::Actor;
use crate::undo::{self, Op};

/// One entry of the file's context.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    /// `about`, `audience`, `rules`, `background`, `fact`… (free).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub by: Actor,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub who: String,
    #[serde(default)]
    pub at: u64,
}

const LOC: &str = "Review › Agent";

pub fn specs() -> Vec<CommandSpec> {
    let c = CommandSpec::new;
    vec![
        c("agent.fileContext", "File Context", LOC, read).params("{} → the file's context entries").pure(),
        // The agent's (no undo step): {op: add|update|remove, …}.
        c("agent.fileContextEdit", "Edit File Context (Agent)", LOC, agent_edit)
            .params(r#"{"op": "add"|"update"|"remove", "id"?, "title"?, "kind"?, "text"?}"#)
            .pure(),
        // The person's (undo steps).
        c("agent.addFileContext", "Add to File Context", LOC, add).params(r#"{"title": string, "kind"?: string, "text": string}"#),
        c("agent.updateFileContext", "Edit File Context", LOC, update)
            .params(r#"{"id": string, "title"?: string, "kind"?: string, "text"?: string}"#),
        c("agent.removeFileContext", "Remove from File Context", LOC, remove).params(r#"{"id": string}"#),
    ]
}

/// A short summary for `get_context` and every question.
pub fn summary(l: &crate::layer::Layer) -> Option<String> {
    if l.file_context.is_empty() {
        return None;
    }
    let mut s = String::new();
    for e in &l.file_context {
        let text: String = e.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
        let kind = if e.kind.is_empty() { String::new() } else { format!(" ({})", e.kind) };
        s.push_str(&format!("{}{kind}: {text}; ", e.title));
        if s.chars().count() > 600 {
            break;
        }
    }
    let s: String = s.trim_end_matches("; ").chars().take(600).collect();
    Some(format!("{} entr{}: {s} (get_file_context for all of it)", l.file_context.len(), if l.file_context.len() == 1 { "y" } else { "ies" }))
}

fn read(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let l = crate::layer(s);
    let list: Vec<Json> = l
        .file_context
        .iter()
        .map(|e| {
            json!({"id": e.id, "title": e.title, "kind": e.kind, "text": crate::links::for_agent(s, &e.text), "by": if e.by == Actor::Agent { "agent".to_string() } else { e.who.clone() }, "at": e.at})
        })
        .collect();
    let out = json!({"entries": list});
    if is_agent(v) { masked(&crate::view(s), v, out) } else { Ok(out) }
}

/// Adds, updates or removes; returns the entry's id.
fn change(s: &mut Session, v: &Json, op: &str, by: Actor) -> Result<String, CmdError> {
    let who = if by == Actor::Agent { "agent".to_string() } else { s.author.clone() };
    match op {
        "add" => {
            let title = text_p(v, "title", 200)?;
            let raw = text_p(v, "text", 20_000)?;
            let text = crate::links::store(s, &raw);
            let kind = opt_text(v, "kind", 40);
            Ok(crate::update(s, |l| {
                let id = l.fresh_id("f");
                l.file_context.push(Entry { id: id.clone(), title, kind, text, by, who, at: crate::now_ms() });
                id
            }))
        }
        "update" => {
            let id = text_p(v, "id", 100)?;
            let title = p::str(v, "title").map(|t| t.chars().take(200).collect::<String>());
            let kind = p::str(v, "kind").map(|t| t.chars().take(40).collect::<String>());
            let text = p::str(v, "text").map(|t| t.chars().take(20_000).collect::<String>()).map(|t| crate::links::store(s, &t));
            let ok = crate::update(s, |l| match l.file_context.iter_mut().find(|e| e.id == id) {
                Some(e) => {
                    if let Some(t) = title {
                        e.title = t;
                    }
                    if let Some(k) = kind {
                        e.kind = k;
                    }
                    if let Some(t) = text {
                        e.text = t;
                    }
                    e.at = crate::now_ms();
                    true
                }
                None => false,
            });
            if ok { Ok(id) } else { Err(bad(format!("no file context entry `{id}`"))) }
        }
        "remove" => {
            let id = text_p(v, "id", 100)?;
            let ok = crate::update(s, |l| {
                let n = l.file_context.len();
                l.file_context.retain(|e| e.id != id);
                l.file_context.len() != n
            });
            if ok { Ok(id) } else { Err(bad(format!("no file context entry `{id}`"))) }
        }
        _ => Err(bad("`op` is add, update or remove")),
    }
}

fn agent_edit(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    crate::ensure(s);
    let op = p::str(v, "op").unwrap_or("").to_string();
    let by = if is_agent(v) { Actor::Agent } else { Actor::Human };
    let id = change(s, v, &op, by)?;
    Ok(json!({"id": id, "op": op}))
}

fn person(s: &mut Session, v: &Json, op: &str) -> CmdResult {
    if is_agent(v) {
        return Err(bad("use add_file_context / update_file_context / remove_file_context"));
    }
    crate::ensure(s);
    let before = crate::layer(s).file_context.clone();
    let id = change(s, v, op, Actor::Human)?;
    let after = crate::layer(s).file_context.clone();
    undo::push(s, Op::FileContext { before, after });
    Ok(json!({"id": id}))
}

fn add(s: &mut Session, v: &Json) -> CmdResult {
    person(s, v, "add")
}
fn update(s: &mut Session, v: &Json) -> CmdResult {
    person(s, v, "update")
}
fn remove(s: &mut Session, v: &Json) -> CmdResult {
    person(s, v, "remove")
}
