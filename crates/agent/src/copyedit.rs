//! A client's copy is editable (the template rendered for a client, `agent.render`).
//!
//! - **Ordinary text** the person edits stays in that copy only: the edited paragraph is kept in
//!   template form (its client values back as `{{placeholders}}`) as an *overlay* of that client's
//!   copy, which stands in for the template paragraph whenever the copy is rendered again. A
//!   template change elsewhere keeps it; a change to the template paragraph itself keeps the
//!   overlay's text and flags it (a conflict). **Move to template** pushes an overlay into the
//!   template (every client then gets it).
//! - **Client values are one block**: copied, deleted or replaced whole, never split. A value from
//!   the file's own data block (`This file's data`) can be edited, and the edit is written back
//!   to that block, so every place that uses it changes; a value from the CRM is read only.
//! - **Logic output** (a loop's repetitions, a condition's branch) is read only for the person.
//! - The agent edits the template as before, logic blocks included.
//!
//! An edit in the copy runs as the command it is; afterwards it is checked and taken back, and
//! what it means (an overlay, a value) is recorded as the person's layer step (`agent.copyEdit`),
//! so its undo step holds the template, never the copy.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use wordcraft_doc::{Block, Document};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Session, p};

use crate::commands::{bad, is_agent};
use crate::undo::{self, Op};

/// The client id of a copy rendered from the file's own data block.
pub const FILE: &str = "file";

/// A paragraph of a client's copy edited by the person (in template form).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub id: String,
    /// The client whose copy it belongs to.
    pub client: String,
    /// The template block it stands in for (found again by `base` if the template moved).
    pub block: usize,
    /// That template block's text when the edit was made.
    pub base: String,
    /// The edited paragraph(s), values as placeholders.
    pub blocks: Vec<Json>,
    /// The template paragraph changed since: the overlay's text is kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conflict: bool,
    #[serde(default)]
    pub at: u64,
}

pub fn specs() -> Vec<CommandSpec> {
    let c = CommandSpec::new;
    vec![
        c("agent.copyEdit", "Edit in Client's Copy", "Review › Agent", copy_edit)
            .params(r#"{"client", "block", "blocks": [paragraph]} — an overlay of that client's copy; {"client": "file", "label", "value"} — a value of the file's data block"#),
        c("agent.moveToTemplate", "Move to Template", "Review › Agent", move_to_template).params(r#"{"id": overlay id} — the copy's edit goes into the template, for every client"#),
        c("agent.discardCopyEdit", "Discard Copy Edit", "Review › Agent", discard).params(r#"{"id": overlay id} — the copy shows the template's text again"#),
        c("agent.fileData", "This File's Data", "Review › Agent", file_data)
            .params(r#"{"data"?: {"client": {...}}} — the file's own client data (a copy rendered from it is editable and writes back to it); no data: read it"#),
    ]
}

fn block_text(b: &Block) -> String {
    match b {
        Block::Para(p) => p.text.replace(wordcraft_doc::para::OBJ, ""),
        Block::Table(_) => String::new(),
    }
}

/// The template block an overlay stands in for now: where it was if its text is the same, else
/// the nearest block with that text, else where it was (a conflict).
fn resolve(template: &Document, o: &Overlay) -> Option<(usize, bool)> {
    let same = |i: usize| template.body.get(i).is_some_and(|b| block_text(b) == o.base);
    if same(o.block) {
        return Some((o.block, false));
    }
    let near = (0..template.body.len()).filter(|i| same(*i)).min_by_key(|i| i.abs_diff(o.block));
    match near {
        Some(i) => Some((i, false)),
        None => (o.block < template.body.len()).then_some((o.block, true)),
    }
}

/// The template with a client's overlays in place, which rendered block came from which template
/// block, and the overlays now in conflict.
pub fn apply(template: &Document, l: &crate::layer::Layer, client: &str) -> (Document, Vec<(usize, bool)>, Vec<String>) {
    let mut subs: Vec<(usize, Vec<Arc<Block>>)> = vec![];
    let mut conflicts = vec![];
    for o in l.overlays.iter().filter(|o| o.client == client) {
        let Some((i, conflict)) = resolve(template, o) else { continue };
        if conflict {
            conflicts.push(o.id.clone());
        }
        let blocks: Vec<Arc<Block>> = o.blocks.iter().filter_map(|b| serde_json::from_value::<Block>(b.clone()).ok()).map(Arc::new).collect();
        if !blocks.is_empty() && !subs.iter().any(|(k, _)| *k == i) {
            subs.push((i, blocks));
        }
    }
    let mut doc = template.clone();
    // Template block index of each block of `doc`, and whether it is an overlay's.
    let mut origin: Vec<(usize, bool)> = (0..template.body.len()).map(|i| (i, false)).collect();
    subs.sort_by_key(|(i, _)| std::cmp::Reverse(*i));
    for (i, blocks) in subs {
        let n = blocks.len();
        if i < doc.body.len() {
            doc.body.splice(i..=i, blocks);
            origin.splice(i..=i, std::iter::repeat_n((i, true), n));
        }
    }
    (doc, origin, conflicts)
}

/// The file's data block, made from the template's placeholders when it has none yet.
pub fn file_data_of(l: &crate::layer::Layer, template: &Document) -> Json {
    if let Some(d) = &l.data {
        return d.clone();
    }
    let mut client = serde_json::Map::new();
    for b in &template.body {
        let t = block_text(b);
        let mut rest = t.as_str();
        while let Some(i) = rest.find("{{client.") {
            let after = rest.get(i + 9..).unwrap_or("");
            let Some(j) = after.find("}}") else { break };
            let key = after.get(..j).unwrap_or("").trim().to_string();
            if !key.is_empty() && !key.contains('.') {
                client.entry(key.clone()).or_insert_with(|| json!(format!("[{key}]")));
            }
            rest = after.get(j..).unwrap_or("");
        }
    }
    json!({"client": client})
}

// ---------------------------------------------------------------- checking an edit in the copy

/// What an edit in the copy means.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    Refuse(String),
    /// The template block `block`'s paragraph(s), edited (template form).
    Overlay {
        block: usize,
        blocks: Vec<Block>,
    },
    /// A value of the file's data block.
    Value {
        label: String,
        text: String,
    },
}

/// Compares the copy before and after a command.
pub fn analyze(before: &[Arc<Block>], after: &[Arc<Block>], r: &crate::Render) -> Outcome {
    let eq = |a: &Arc<Block>, b: &Arc<Block>| Arc::ptr_eq(a, b) || a == b;
    let mut i0 = 0;
    while i0 < before.len() && i0 < after.len() && before.get(i0).zip(after.get(i0)).is_some_and(|(a, b)| eq(a, b)) {
        i0 += 1;
    }
    let mut k = 0;
    while k < before.len().saturating_sub(i0)
        && k < after.len().saturating_sub(i0)
        && before.get(before.len() - 1 - k).zip(after.get(after.len() - 1 - k)).is_some_and(|(a, b)| eq(a, b))
    {
        k += 1;
    }
    let (mut b0, b1, mut a0, a1) = (i0, before.len() - k, i0, after.len() - k);
    if b0 == b1 && a0 == a1 {
        return Outcome::Nothing;
    }
    // A paragraph added between two: the one before it changed (Enter at its end).
    if b0 == b1 {
        if b0 == 0 {
            return Outcome::Refuse("add text inside a paragraph of the client's copy".into());
        }
        b0 -= 1;
        a0 -= 1;
    }
    // One template paragraph, shown once.
    let srcs: Vec<&crate::template::Source> = (b0..b1).filter_map(|i| r.map.get(i)).collect();
    if srcs.len() != b1 - b0 || srcs.is_empty() {
        return Outcome::Refuse("this part of the client's copy can't be edited".into());
    }
    if srcs.iter().any(|s| !s.loops.is_empty() || s.logic) {
        return Outcome::Refuse(
            "this text comes from a loop or a condition of the template: edit the template (Show: Template) or ask the agent".into(),
        );
    }
    let block = srcs.first().map_or(0, |s| s.block);
    if srcs.iter().any(|s| s.block != block) {
        return Outcome::Refuse("edit one paragraph at a time in a client's copy".into());
    }
    let paras: Option<Vec<&wordcraft_doc::Paragraph>> = (b0..b1).map(|i| before.get(i).and_then(|b| b.as_para())).collect();
    let news: Option<Vec<&wordcraft_doc::Paragraph>> = (a0..a1).map(|i| after.get(i).and_then(|b| b.as_para())).collect();
    let (Some(paras), Some(news)) = (paras, news) else {
        return Outcome::Refuse("tables in a client's copy are edited in the template".into());
    };
    // The client values in it, in order: (label, text).
    let mut values: Vec<(String, String, usize, usize, usize)> = vec![];
    for (k, i) in (b0..b1).enumerate() {
        for f in r.filled.iter().filter(|f| f.path.0.first().copied() == Some(i as u32) && f.path.0.len() == 1) {
            let text = paras.get(k).and_then(|p| p.text.get(f.start..f.end)).unwrap_or("").to_string();
            values.push((f.label.clone(), text, k, f.start, f.end));
        }
    }
    // One paragraph to one: a change inside a value edits it; one that splits it is refused.
    if let ([old], [new]) = (paras.as_slice(), news.as_slice()) {
        let (bt, at) = (old.text.as_str(), new.text.as_str());
        let pre = bt.bytes().zip(at.bytes()).take_while(|(x, y)| x == y).count();
        let suf =
            bt.bytes().rev().zip(at.bytes().rev()).take_while(|(x, y)| x == y).count().min(bt.len().min(at.len()) - pre.min(bt.len().min(at.len())));
        let (cs, ce) = (pre, bt.len() - suf);
        let delta = at.len() as isize - bt.len() as isize;
        for (label, _, _, fs, fe) in &values {
            let inside = *fs <= cs && ce <= *fe;
            let disjoint = ce <= *fs || cs >= *fe;
            let covers = cs <= *fs && *fe <= ce;
            if inside && cs < ce.max(cs + 1) && (cs != ce || delta != 0) {
                if r.client != FILE {
                    return Outcome::Refuse("this value comes from the CRM: it can't be changed here".into());
                }
                let end = (*fe as isize + delta).max(*fs as isize) as usize;
                let text = at.get(*fs..end).unwrap_or("").to_string();
                return Outcome::Value { label: label.clone(), text };
            }
            if !disjoint && !covers {
                return Outcome::Refuse("client data is one block: select all of it to delete or replace it".into());
            }
        }
    }
    // An ordinary edit: the paragraph(s) in template form, values back as placeholders.
    let mut out: Vec<Block> = vec![];
    let mut next = 0usize;
    for np in &news {
        let mut p = (*np).clone();
        let mut found: Vec<(usize, usize, String)> = vec![];
        let mut from = 0usize;
        while let Some((label, text, ..)) = values.get(next) {
            if text.is_empty() {
                next += 1;
                continue;
            }
            match p.text.get(from..).and_then(|t| t.find(text.as_str())) {
                Some(i) => {
                    found.push((from + i, from + i + text.len(), label.clone()));
                    from = from + i + text.len();
                    next += 1;
                }
                None => break,
            }
        }
        for (s, e, label) in found.into_iter().rev() {
            let props = p.props_at(s).clone();
            if p.delete(s, e).is_ok() {
                let _ = p.insert_text(s, &format!("{{{{{label}}}}}"), &props);
            }
        }
        p.touch();
        out.push(Block::Para(p));
    }
    Outcome::Overlay { block, blocks: out }
}

// ---------------------------------------------------------------- commands (the person's)

fn person(v: &Json) -> Result<(), CmdError> {
    if is_agent(v) {
        return Err(bad("the agent edits the template, not a client's copy"));
    }
    Ok(())
}

fn copy_edit(s: &mut Session, v: &Json) -> CmdResult {
    person(v)?;
    crate::ensure(s);
    let client = p::str(v, "client").unwrap_or("").to_string();
    if client.is_empty() {
        return Err(bad("`client` is required"));
    }
    let before = (crate::layer(s).overlays.clone(), crate::layer(s).data.clone());
    if let Some(label) = p::str(v, "label") {
        // A value of the file's data block: written back there.
        if client != FILE {
            return Err(bad("values from the CRM are read only"));
        }
        let value = p::str(v, "value").unwrap_or("").chars().take(2_000).collect::<String>();
        let path: Vec<String> = label.split('.').map(str::to_string).collect();
        let template = crate::agent_doc(s).clone();
        let l = crate::layer(s);
        let mut data = file_data_of(&l, &template);
        let mut at = &mut data;
        for (k, key) in path.iter().enumerate() {
            if !at.is_object() {
                *at = json!({});
            }
            let Some(o) = at.as_object_mut() else { break };
            if k + 1 == path.len() {
                o.insert(key.clone(), json!(value));
                break;
            }
            at = o.entry(key.clone()).or_insert_with(|| json!({}));
        }
        crate::update(s, |l| l.data = Some(data));
    } else {
        let block = p::u64(v, "block").map(|n| n.min(usize::MAX as u64) as usize).ok_or_else(|| bad("`block` is required"))?;
        let blocks: Vec<Json> = v.get("blocks").and_then(Json::as_array).cloned().unwrap_or_default();
        if blocks.is_empty() || blocks.len() > 50 || blocks.iter().any(|b| serde_json::from_value::<Block>(b.clone()).is_err()) {
            return Err(bad("`blocks` are the edited paragraphs"));
        }
        let base = crate::agent_doc(s).body.get(block).map(|b| block_text(b)).ok_or_else(|| bad("no such template block"))?;
        crate::update(s, |l| {
            let id = l.overlays.iter().find(|o| o.client == client && o.block == block).map(|o| o.id.clone()).unwrap_or_else(|| l.fresh_id("e"));
            l.overlays.retain(|o| o.id != id);
            l.overlays.push(Overlay { id, client: client.clone(), block, base, blocks, conflict: false, at: crate::now_ms() });
        });
    }
    let after = (crate::layer(s).overlays.clone(), crate::layer(s).data.clone());
    undo::push(s, Op::CopyEdits { before: Box::new(before), after: Box::new(after) });
    Ok(json!({"client": client}))
}

fn move_to_template(s: &mut Session, v: &Json) -> CmdResult {
    person(v)?;
    crate::ensure(s);
    let id = p::str(v, "id").unwrap_or("").to_string();
    let l = crate::layer(s);
    let o = l.overlays.iter().find(|o| o.id == id).cloned().ok_or_else(|| bad(format!("no copy edit `{id}`")))?;
    // The observer has put the template on screen for this command.
    if crate::render_state(s).is_some() {
        return Err(bad("the template isn't available"));
    }
    let (i, _) = resolve(&s.doc, &o).ok_or_else(|| bad("its paragraph is no longer in the template"))?;
    let blocks: Vec<Arc<Block>> = o.blocks.iter().filter_map(|b| serde_json::from_value::<Block>(b.clone()).ok()).map(Arc::new).collect();
    if i >= s.doc.body.len() || blocks.is_empty() {
        return Err(bad("its paragraph is no longer in the template"));
    }
    s.doc.body.splice(i..=i, blocks);
    s.touch();
    let before = (l.overlays.clone(), l.data.clone());
    crate::update(s, |l| l.overlays.retain(|x| x.id != id));
    let after = (crate::layer(s).overlays.clone(), crate::layer(s).data.clone());
    undo::push(s, Op::CopyEdits { before: Box::new(before), after: Box::new(after) });
    Ok(json!({"moved": id, "block": i}))
}

fn discard(s: &mut Session, v: &Json) -> CmdResult {
    person(v)?;
    crate::ensure(s);
    let id = p::str(v, "id").unwrap_or("").to_string();
    let l = crate::layer(s);
    if !l.overlays.iter().any(|o| o.id == id) {
        return Err(bad(format!("no copy edit `{id}`")));
    }
    let before = (l.overlays.clone(), l.data.clone());
    crate::update(s, |l| l.overlays.retain(|x| x.id != id));
    let after = (crate::layer(s).overlays.clone(), crate::layer(s).data.clone());
    undo::push(s, Op::CopyEdits { before: Box::new(before), after: Box::new(after) });
    Ok(json!({"discarded": id}))
}

fn file_data(s: &mut Session, v: &Json) -> CmdResult {
    crate::ensure(s);
    match v.get("data").filter(|d| d.is_object()) {
        Some(d) => {
            person(v)?;
            let before = (crate::layer(s).overlays.clone(), crate::layer(s).data.clone());
            let d = d.clone();
            crate::update(s, |l| l.data = Some(d));
            let after = (crate::layer(s).overlays.clone(), crate::layer(s).data.clone());
            undo::push(s, Op::CopyEdits { before: Box::new(before), after: Box::new(after) });
            Ok(json!({"saved": true}))
        }
        None => {
            if is_agent(v) {
                // The values are client data: the agent sees the placeholders only.
                return Err(bad("the file's data is client data: you see its placeholders in the template"));
            }
            let l = crate::layer(s);
            Ok(file_data_of(&l, crate::agent_doc(s)))
        }
    }
}

/// The copy's paragraphs that are the person's edits (`(rendered block, overlay id, conflict)`).
pub fn edited_blocks(s: &Session) -> Vec<(usize, String, bool)> {
    let Some(r) = crate::render_state(s) else { return vec![] };
    if r.proposal.is_some() {
        return vec![];
    }
    let l = crate::layer(s);
    r.map
        .iter()
        .enumerate()
        .filter(|(_, src)| src.overlay)
        .filter_map(|(i, src)| {
            l.overlays
                .iter()
                .find(|o| o.client == r.client && resolve(&r.template, o).is_some_and(|(b, _)| b == src.block))
                .map(|o| (i, o.id.clone(), o.conflict))
        })
        .collect()
}
