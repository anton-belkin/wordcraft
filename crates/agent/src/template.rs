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

/// Where a block of the rendered document came from (one per top-level body block).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// Its block in the template's body.
    pub block: usize,
    /// The loops it was repeated by, outermost first: each `(for tag, endfor tag)` block indices.
    pub loops: Vec<(usize, usize)>,
    /// Its iteration of each of those loops.
    pub iterations: Vec<usize>,
    /// The placeholders filled in it, in order: `(paragraph path inside the block, start, end in
    /// the rendered text, length of the placeholder in the template)`.
    pub edits: Vec<(Vec<u32>, usize, usize, usize)>,
    /// Inside a loop or a condition: the template's logic made it.
    pub logic: bool,
    /// From a person's edit of this client's copy (an overlay), not from the template's text.
    pub overlay: bool,
}

/// Which side of a filled-in value a position inside it goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Start,
    End,
}

/// A position of the rendered document in the template: its path and offset, and the loops whose
/// repetition it is in (empty: exactly one template paragraph). Offsets inside a filled-in value go
/// to the start or end of its placeholder.
pub fn to_template<'a>(map: &'a [Source], path: &Path, off: usize, bias: Bias) -> Option<(Path, usize, &'a [(usize, usize)])> {
    let (first, sub) = path.0.split_first()?;
    let src = map.get(*first as usize)?;
    // How much longer the rendered text is than the template's, up to the position.
    let mut delta: isize = 0;
    let mut t_off = None;
    for (_, start, end, tlen) in src.edits.iter().filter(|e| e.0.as_slice() == sub) {
        if off <= *start {
            break;
        }
        let t_start = (*start as isize - delta).max(0) as usize;
        if off < *end {
            t_off = Some(if bias == Bias::Start { t_start } else { t_start + tlen });
            break;
        }
        delta += *end as isize - *start as isize - *tlen as isize;
    }
    let t_off = t_off.unwrap_or_else(|| (off as isize - delta).max(0) as usize);
    let mut out = vec![src.block as u32];
    out.extend_from_slice(sub);
    Some((Path(out), t_off, &src.loops))
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
    let clean: String = p.text.chars().filter(|c| *c != wordcraft_doc::para::OBJ).collect();
    let t = clean.trim();
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
/// Where the blocks of the rendered document come from, as they are made.
#[derive(Default)]
struct Trace {
    /// The template index of `blocks[0]`.
    base: usize,
    loops: Vec<(usize, usize)>,
    iterations: Vec<usize>,
    /// Conditions entered.
    ifs: usize,
    map: Vec<Source>,
    filled: Vec<(usize, Vec<u32>, usize, usize, String)>,
}

fn expand(blocks: &[Arc<Block>], scope: &Map<String, Json>, depth: usize, out: &mut Blocks, tr: &mut Trace) {
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
                    let base = tr.base;
                    tr.loops.push((base + i, base + end));
                    for (n, item) in items.iter().take(1_000).enumerate() {
                        let mut inner = scope.clone();
                        inner.insert(var.clone(), item.clone());
                        tr.iterations.push(n);
                        tr.base = base + i + 1;
                        expand(body, &inner, depth + 1, out, tr);
                        tr.iterations.pop();
                    }
                    tr.base = base;
                    tr.loops.pop();
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
                let base = tr.base;
                tr.base = base + a;
                tr.ifs += 1;
                expand(blocks.get(a..z).unwrap_or(&[]), scope, depth + 1, out, tr);
                tr.ifs -= 1;
                tr.base = base;
                i = end + 1;
            }
            // A stray closing tag shows as written.
            _ => {
                let mut b = (**b).clone();
                let at = out.len();
                let mut src = Source {
                    block: tr.base + i,
                    loops: tr.loops.clone(),
                    iterations: tr.iterations.clone(),
                    edits: vec![],
                    logic: !tr.loops.is_empty() || tr.ifs > 0,
                    overlay: false,
                };
                let filled = &mut tr.filled;
                fill_block(&mut b, scope, &mut vec![], &mut |path, s, e, tlen, l| {
                    src.edits.push((path.clone(), s, e, tlen));
                    filled.push((at, path, s, e, l));
                });
                tr.map.push(src);
                out.push(Arc::new(b));
                i += 1;
            }
        }
    }
}

/// Fills the inline placeholders of a block (a paragraph, or every paragraph of a table).
fn fill_block(b: &mut Block, scope: &Map<String, Json>, sub: &mut Vec<u32>, rec: &mut dyn FnMut(Vec<u32>, usize, usize, usize, String)) {
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
                            out.push((open, open + value.len(), close - open, name));
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
            for (s, e, tlen, l) in out {
                rec(sub.clone(), s, e, tlen, l);
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
    let (doc, filled, _) = render_mapped(template, data);
    (doc, filled)
}

/// [`render`], with where each rendered body block came from in the template.
pub fn render_mapped(template: &Document, data: &Json) -> (Document, Vec<Filled>, Vec<Source>) {
    let scope = data.as_object().cloned().unwrap_or_default();
    let mut out = Vec::new();
    let mut tr = Trace::default();
    expand(&template.body, &scope, 0, &mut out, &mut tr);
    let mut doc = template.clone();
    doc.body = out;
    doc.ensure_nonempty();
    let filled = tr
        .filled
        .into_iter()
        .map(|(block, sub, start, end, label)| {
            let mut path = vec![block as u32];
            path.extend(sub);
            Filled { path: Path(path), start, end, label }
        })
        .collect();
    (doc, filled, tr.map)
}
