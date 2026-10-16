//! Editing paragraphs in one go (`edit_paragraphs`, like GridCraft's `set_ranges`) and find and
//! replace on the agent's view (`replace_text`).
//!
//! `edit_paragraphs {reason, edits: [{at, text?, style?, list?: {kind, level?, restart?},
//! insertAfter?: [text | {text, style?, list?}]}]}` replaces passages or whole paragraphs, sets
//! their style, makes them (or takes them out of) a bulleted or numbered list, and adds paragraphs
//! after them — one undo step, or with `topic` one step of the topic's proposal. Edits name
//! paragraphs as they are before the call; they are applied from the last to the first, so the
//! numbers hold.
//!
//! `replace_text {find, replace, all?, paragraph?, reason}` finds text in what the agent reads (client
//! data stays out of reach) and replaces it: one undo step, or one step of a proposal.

use serde_json::{Value as Json, json};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::commands::{bad, doc_check, exec_at, gate, opt_text, text_p};

const LOC: &str = "Review › Agent";

pub fn specs() -> Vec<CommandSpec> {
    let c = CommandSpec::new;
    vec![
        c("agent.editParagraphs", "Edit Paragraphs (Agent)", LOC, edit_paragraphs)
            .params(r#"{"reason", "source"?, "topic"?, "option"?, "edits": [{"at": {paragraph, quote?, occurrence?, through?}, "text"?: string, "style"?: string, "list"?: {"kind": "bullet"|"number"|"none", "level"?: 0-8, "restart"?: bool}, "insertAfter"?: [string | {"text", "style"?, "list"?}]}]} — one undo step"#)
            .pure(),
        c("agent.replaceText", "Replace Text (Agent)", LOC, replace_text)
            .params(r#"{"find": string, "replace": string, "all"?: bool, "paragraph"?: number, "reason", "source"?, "topic"?, "option"?} — in the agent's view; one undo step"#)
            .pure(),
    ]
}

fn reason_of(v: &Json) -> Result<(String, String), CmdError> {
    let reason = text_p(v, "reason", 4_000).map_err(|_| bad("`reason` is required: say why you are changing the document"))?;
    Ok((reason, opt_text(v, "source", 2_000)))
}

/// In a topic: one step of its proposal; else on the document (the template under a client's
/// copy), one undo step.
fn run_or_propose(
    s: &mut Session,
    v: &Json,
    command: &str,
    params: Json,
    first_at: Json,
    f: fn(&mut Session, &Json, &str, &str) -> CmdResult,
) -> CmdResult {
    let (reason, source) = reason_of(v)?;
    if v.get("topic").is_some_and(|t| !t.is_null()) && !crate::commands::in_agent_exec() {
        let mut w = v.clone();
        w["at"] = first_at;
        return crate::proposals::propose(s, &w, command, params, reason, source);
    }
    let swapped = if crate::commands::in_agent_exec() { None } else { crate::template_in(s) };
    let reader = s.sel.clone();
    let r = f(s, &params, &reason, &source);
    s.sel = wordcraft_engine::Selection { anchor: s.doc.clamp(&reader.anchor), focus: s.doc.clamp(&reader.focus) };
    crate::template_out(s, swapped);
    r
}

/// Runs one command on the agent's terms; every one after the first joins the same undo step.
fn step(s: &mut Session, first: &mut bool, command: &str, params: Json, at: Json, reason: &str, source: &str) -> Result<(), CmdError> {
    if !*first {
        s.join_next_undo();
    }
    *first = false;
    exec_at(s, command, params, &json!({"at": at, "actor": "agent"}), reason.to_string(), source.to_string()).map(|_| ())
}

/// A list on paragraphs `a..=b` (taken off first, so the toggles don't turn it off).
fn set_list(s: &mut Session, first: &mut bool, a: usize, b: usize, list: &Json, reason: &str, source: &str) -> Result<(), CmdError> {
    let at = json!({"paragraph": a, "through": b});
    let kind = p::str(list, "kind").unwrap_or("number");
    step(s, first, "para.bullets", json!({"off": true}), at.clone(), reason, source)?;
    match kind {
        "none" => return Ok(()),
        "bullet" | "bullets" => step(s, first, "para.bullets", json!({}), at.clone(), reason, source)?,
        "number" | "numbered" | "numbers" => step(s, first, "para.numbering", json!({"kind": "numbered"}), at.clone(), reason, source)?,
        k => step(s, first, "para.numbering", json!({"kind": k}), at.clone(), reason, source)?,
    }
    if let Some(level) = p::u64(list, "level").filter(|l| *l > 0) {
        step(s, first, "para.listLevel", json!({"level": level.min(8)}), at.clone(), reason, source)?;
    }
    if p::bool(list, "restart").unwrap_or(false) {
        step(s, first, "para.restartNumbering", json!({}), json!({"paragraph": a}), reason, source)?;
    }
    Ok(())
}

fn check_text(t: &str) -> Result<(), CmdError> {
    if t.contains('\n') || t.contains('\r') {
        return Err(bad("one paragraph per text: put further paragraphs in `insertAfter`"));
    }
    Ok(())
}

// ---------------------------------------------------------------- edit_paragraphs

fn edit_paragraphs(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let edits =
        v.get("edits").and_then(Json::as_array).cloned().ok_or_else(|| bad("`edits` is a list of {at, text?, style?, list?, insertAfter?}"))?;
    if edits.is_empty() || edits.len() > 100 {
        return Err(bad("give 1 to 100 edits"));
    }
    // Each target with its paragraph's text, so a proposal finds it again later.
    let view = crate::view(s);
    let mut annotated = vec![];
    for e in &edits {
        let at = e.get("at").filter(|a| a.is_object()).cloned().ok_or_else(|| bad("each edit needs `at` {paragraph, quote?}"))?;
        let sp = crate::commands::place_in(&view, &at)?.ok_or_else(|| bad("`at` needs `paragraph` and/or `quote`"))?;
        let mut at = at;
        at["paragraph"] = json!(sp.para);
        at["paraText"] = json!(view.para(sp.para).map(|m| m.text.clone()).unwrap_or_default());
        let mut e = e.clone();
        e["at"] = at;
        annotated.push(e);
    }
    let first_at = annotated.first().and_then(|e| e.get("at")).cloned().unwrap_or_default();
    let mut params = json!({"edits": annotated});
    params["reason"] = json!(opt_text(v, "reason", 4_000));
    run_or_propose(s, v, "agent.editParagraphs", params, first_at, apply_edits)
}

fn apply_edits(s: &mut Session, params: &Json, reason: &str, source: &str) -> CmdResult {
    let edits = params.get("edits").and_then(Json::as_array).cloned().unwrap_or_default();
    // Where each edit is now (found again by its text if the document moved), last first.
    let view = crate::view(s);
    let mut targets = vec![];
    for (i, e) in edits.iter().enumerate() {
        let at = e.get("at").cloned().unwrap_or_default();
        let pt = p::str(&at, "paraText").unwrap_or("").to_string();
        let found = crate::proposals::locate(&view, &at, &pt).ok_or_else(|| bad(format!("edit {}: its text is no longer in the document", i + 1)))?;
        let first = p::u64(&found, "paragraph").unwrap_or(0) as usize;
        let last = p::u64(&found, "through").map_or(first, |t| t as usize).max(first);
        targets.push((first, last, found, e.clone()));
    }
    targets.sort_by_key(|t| std::cmp::Reverse(t.0));
    let mut first_step = true;
    let mut changed = 0usize;
    for (a, b, at, e) in targets {
        if let Some(text) = p::str(&e, "text") {
            check_text(text)?;
            if text.is_empty() {
                step(s, &mut first_step, "text.delete", json!({}), at.clone(), reason, source)?;
            } else {
                step(s, &mut first_step, "text.insert", json!({"text": text}), at.clone(), reason, source)?;
            }
        }
        // Several paragraphs replaced by one text: now one.
        let b = if p::str(&e, "text").is_some() && p::u64(&at, "through").is_some() { a } else { b };
        if let Some(style) = p::str(&e, "style") {
            step(s, &mut first_step, "para.style", json!({"style": style}), json!({"paragraph": a, "through": b}), reason, source)?;
        }
        if let Some(list) = e.get("list").filter(|l| l.is_object()) {
            set_list(s, &mut first_step, a, b, list, reason, source)?;
        }
        // New paragraphs after it: each continues the one before (a list goes on).
        let mut after = b;
        for item in e.get("insertAfter").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(200) {
            let (text, style, list) = match item {
                Json::String(t) => (t.clone(), None, None),
                o => (
                    p::str(o, "text").unwrap_or("").to_string(),
                    p::str(o, "style").map(str::to_string),
                    o.get("list").filter(|l| l.is_object()).cloned(),
                ),
            };
            check_text(&text)?;
            step(s, &mut first_step, "text.newParagraph", json!({}), json!({"paragraph": after, "caret": "end"}), reason, source)?;
            after += 1;
            if !text.is_empty() {
                step(s, &mut first_step, "text.insert", json!({"text": text}), json!({"paragraph": after, "caret": "start"}), reason, source)?;
            }
            if let Some(st) = style {
                step(s, &mut first_step, "para.style", json!({"style": st}), json!({"paragraph": after}), reason, source)?;
            }
            if let Some(l) = list {
                set_list(s, &mut first_step, after, after, &l, reason, source)?;
            }
        }
        changed += 1;
    }
    Ok(json!({"ok": true, "edits": changed, "hint": "one undo step"}))
}

// ---------------------------------------------------------------- replace_text

fn replace_text(s: &mut Session, v: &Json) -> CmdResult {
    gate(v)?;
    doc_check(s, v)?;
    let find = text_p(v, "find", 2_000)?;
    let replace =
        p::str(v, "replace").map(|t| t.chars().take(20_000).collect::<String>()).ok_or_else(|| bad("`replace` is required (\"\" deletes)"))?;
    check_text(&replace)?;
    let para = p::u64(v, "paragraph").map(|n| n.min(usize::MAX as u64) as usize);
    let all = p::bool(v, "all").unwrap_or(false);
    let view = crate::view(s);
    let mut hits = vec![];
    for k in 0..1_000 {
        match view.find(para, Some(&find), k) {
            Ok(sp) => hits.push(sp),
            Err(_) => break,
        }
    }
    if hits.is_empty() {
        return Err(bad(format!("`{find}` isn't in the document as you read it (client data can't be searched)")));
    }
    if hits.len() > 1 && !all {
        let where_: Vec<String> = hits.iter().take(10).map(|h| format!("§{}", h.para)).collect();
        return Err(bad(format!("`{find}` is there {} times ({}): give `paragraph`, or `all: true`", hits.len(), where_.join(", "))));
    }
    let first_at = hits.first().map(|h| json!({"paragraph": h.para, "quote": find})).unwrap_or_default();
    let mut params = json!({"find": find, "replace": replace, "all": all, "reason": opt_text(v, "reason", 4_000)});
    if let Some(n) = para {
        params["paragraph"] = json!(n);
    }
    run_or_propose(s, v, "agent.replaceText", params, first_at, apply_replace)
}

fn apply_replace(s: &mut Session, params: &Json, reason: &str, source: &str) -> CmdResult {
    let find = p::str(params, "find").unwrap_or("").to_string();
    let replace = p::str(params, "replace").unwrap_or("").to_string();
    let para = p::u64(params, "paragraph").map(|n| n as usize);
    let all = p::bool(params, "all").unwrap_or(false);
    let view = crate::view(s);
    let mut hits: Vec<(usize, usize)> = vec![];
    for k in 0..1_000 {
        let Ok(sp) = view.find(para, Some(&find), k) else { break };
        let occ = hits.iter().filter(|(p, _)| *p == sp.para).count();
        hits.push((sp.para, occ));
        if !all {
            break;
        }
    }
    if hits.is_empty() {
        return Err(bad(format!("`{find}` isn't in the document any more")));
    }
    let n = hits.len();
    let mut first = true;
    // Last first: earlier matches keep their places.
    for (p, occ) in hits.into_iter().rev() {
        let at = json!({"paragraph": p, "quote": find, "occurrence": occ});
        if replace.is_empty() {
            step(s, &mut first, "text.delete", json!({}), at, reason, source)?;
        } else {
            step(s, &mut first, "text.insert", json!({"text": replace}), at, reason, source)?;
        }
    }
    Ok(json!({"ok": true, "replaced": n}))
}
