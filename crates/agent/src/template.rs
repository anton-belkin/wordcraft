//! Templates: a document with `{{client.name}}` placeholders and Jinja-like blocks, rendered with a
//! client's data from the [`crm`](crate::crm).
//!
//! - Inline: `{{client.name}}`, `{{ubo.share}}` (a dotted path into the data; spaces allowed).
//! - Blocks, each tag alone in its own body paragraph: `{% for ubo in client.ubos %}` …
//!   `{% endfor %}`, `{% if client.vip %}` … `{% else %}` … `{% endif %}` (`not` allowed).
//!   Blocks repeat or drop whole paragraphs and tables, and nest.
//!
//! An unknown placeholder stays as written, so it shows in the rendered document.

use std::sync::Arc;

use serde_json::{Map, Value as Json};
use wordcraft_doc::{Block, Blocks, Document, Path, StoryRef};

/// A value filled into the rendered document: where, and which placeholder it replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filled {
    pub path: Path,
    pub start: usize,
    pub end: usize,
    pub label: String,
}

/// Deepest nesting of blocks followed.
const MAX_DEPTH: usize = 16;
/// Most body blocks a render makes (a loop over a long list stops there).
const MAX_BLOCKS: usize = 20_000;

enum Tag {
    For { var: String, list: String },
    EndFor,
    If { cond: String, negate: bool },
    Else,
    EndIf,
}

fn tag_of(b: &Block) -> Option<Tag> {
    let p = b.as_para()?;
    let t = p.text.trim();
    let inner = t.strip_prefix("{%")?.strip_suffix("%}")?.trim();
    let words: Vec<&str> = inner.split_whitespace().collect();
    match words.as_slice() {
        ["for", var, "in", list] => Some(Tag::For { var: (*var).to_string(), list: (*list).to_string() }),
        ["endfor"] => Some(Tag::EndFor),
        ["if", "not", cond] => Some(Tag::If { cond: (*cond).to_string(), negate: true }),
        ["if", cond] => Some(Tag::If { cond: (*cond).to_string(), negate: false }),
        ["else"] => Some(Tag::Else),
        ["endif"] => Some(Tag::EndIf),
        _ => None,
    }
}

/// Whether the document uses template syntax at all.
pub fn is_template(doc: &Document) -> bool {
    doc.para_paths(StoryRef::Body).iter().any(|p| doc.para(StoryRef::Body, p).is_some_and(|x| x.text.contains("{{") || x.text.contains("{%")))
}

/// The index of the tag closing the block opened at `open` (`EndFor` or `EndIf`), and of a
/// matching `Else`.
fn closing(blocks: &[Arc<Block>], open: usize) -> (Option<usize>, Option<usize>) {
    let mut depth = 0usize;
    let mut els = None;
    for (i, b) in blocks.iter().enumerate().skip(open + 1) {
        match tag_of(b) {
            Some(Tag::For { .. } | Tag::If { .. }) => depth += 1,
            Some(Tag::EndFor | Tag::EndIf) if depth == 0 => return (Some(i), els),
            Some(Tag::EndFor | Tag::EndIf) => depth -= 1,
            Some(Tag::Else) if depth == 0 => els = Some(i),
            _ => {}
        }
    }
    (None, els)
}

fn lookup<'a>(scope: &'a Map<String, Json>, path: &str) -> Option<&'a Json> {
    let mut parts = path.split('.');
    let mut v = scope.get(parts.next()?)?;
    for k in parts {
        v = match v {
            Json::Object(o) => o.get(k)?,
            Json::Array(a) => a.get(k.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(v)
}

fn truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::String(s)) => !s.is_empty(),
        Some(Json::Array(a)) => !a.is_empty(),
        Some(Json::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Json::Object(o)) => !o.is_empty(),
    }
}

fn text_of(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Bool(b) => if *b { "yes" } else { "no" }.into(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// The blocks of `blocks` with loops and conditions applied; inline placeholders are filled as the
/// blocks are copied (each copy with its own loop variables).
fn expand(
    blocks: &[Arc<Block>],
    scope: &Map<String, Json>,
    depth: usize,
    out: &mut Blocks,
    filled: &mut Vec<(usize, Vec<u32>, usize, usize, String)>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut i = 0;
    while i < blocks.len() && out.len() < MAX_BLOCKS {
        let Some(b) = blocks.get(i) else { break };
        match tag_of(b) {
            Some(Tag::For { var, list }) => {
                let (end, _) = closing(blocks, i);
                let end = end.unwrap_or(blocks.len());
                let body = blocks.get(i + 1..end).unwrap_or(&[]);
                if let Some(Json::Array(items)) = lookup(scope, &list) {
                    for item in items.iter().take(1_000) {
                        let mut inner = scope.clone();
                        inner.insert(var.clone(), item.clone());
                        expand(body, &inner, depth + 1, out, filled);
                    }
                }
                i = end + 1;
            }
            Some(Tag::If { cond, negate }) => {
                let (end, els) = closing(blocks, i);
                let end = end.unwrap_or(blocks.len());
                let yes = truthy(lookup(scope, &cond)) != negate;
                let (a, z) = match (yes, els) {
                    (true, Some(e)) => (i + 1, e),
                    (true, None) => (i + 1, end),
                    (false, Some(e)) => (e + 1, end),
                    (false, None) => (end, end),
                };
                expand(blocks.get(a..z).unwrap_or(&[]), scope, depth + 1, out, filled);
                i = end + 1;
            }
            // A stray closing tag shows as written.
            _ => {
                let mut b = (**b).clone();
                let at = out.len();
                fill_block(&mut b, scope, &mut vec![], &mut |path, s, e, l| filled.push((at, path, s, e, l)));
                out.push(Arc::new(b));
                i += 1;
            }
        }
    }
}

/// Fills the inline placeholders of a block (a paragraph, or every paragraph of a table).
fn fill_block(b: &mut Block, scope: &Map<String, Json>, sub: &mut Vec<u32>, rec: &mut dyn FnMut(Vec<u32>, usize, usize, String)) {
    match b {
        Block::Para(p) => {
            if !p.text.contains("{{") {
                return;
            }
            let mut out = Vec::new();
            let mut search = 0usize;
            while let Some(rel) = p.text.get(search..).and_then(|t| t.find("{{")) {
                let open = search + rel;
                let Some(close_rel) = p.text.get(open..).and_then(|t| t.find("}}")) else { break };
                let close = open + close_rel + 2;
                let name = p.text.get(open + 2..close - 2).unwrap_or("").trim().to_string();
                match lookup(scope, &name) {
                    Some(v) if !name.is_empty() => {
                        let value = text_of(v);
                        let props = p.props_at(open).clone();
                        if p.delete(open, close).is_ok() && p.insert_text(open, &value, &props).is_ok() {
                            out.push((open, open + value.len(), name));
                            search = open + value.len();
                        } else {
                            search = close;
                        }
                    }
                    _ => search = close,
                }
            }
            if !out.is_empty() {
                p.touch();
            }
            for (s, e, l) in out {
                rec(sub.clone(), s, e, l);
            }
        }
        Block::Table(t) => {
            for (ri, row) in t.rows.iter_mut().enumerate() {
                for (ci, cell) in row.cells.iter_mut().enumerate() {
                    for (bi, blk) in cell.blocks.iter_mut().enumerate() {
                        sub.extend([ri as u32, ci as u32, bi as u32]);
                        fill_block(Arc::make_mut(blk), scope, sub, rec);
                        sub.truncate(sub.len().saturating_sub(3));
                    }
                }
            }
        }
    }
}

/// `template` rendered with `data` (`{"client": …}`), and where values were filled in.
pub fn render(template: &Document, data: &Json) -> (Document, Vec<Filled>) {
    let scope = data.as_object().cloned().unwrap_or_default();
    let mut out = Vec::new();
    let mut raw = Vec::new();
    expand(&template.body, &scope, 0, &mut out, &mut raw);
    let mut doc = template.clone();
    doc.body = out;
    doc.ensure_nonempty();
    let filled = raw
        .into_iter()
        .map(|(block, sub, start, end, label)| {
            let mut path = vec![block as u32];
            path.extend(sub);
            Filled { path: Path(path), start, end, label }
        })
        .collect();
    (doc, filled)
}
