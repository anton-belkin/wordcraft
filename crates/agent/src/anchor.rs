//! Anchors: text ranges found again by their text after edits.
//!
//! An anchor keeps the paragraph it was last seen in, the byte offsets there and the quoted text.
//! [`resolve`] looks for the quote at those offsets, then elsewhere in the same paragraph, then in
//! the paragraphs nearby, then by the quote's first and last words (an edit inside it). Nothing is
//! written into the document, so the anchors survive any edit, undo, and a trip through Word.

use wordcraft_doc::{Document, Path, Pos, StoryRef};

use crate::layer::Anchor;

/// A resolved range in the body: one paragraph, byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Index of the paragraph among the body's paragraphs (tables included), in document order.
    pub para: usize,
    pub path: Path,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn start_pos(&self) -> Pos {
        Pos::new(StoryRef::Body, self.path.clone(), self.start)
    }
    pub fn end_pos(&self) -> Pos {
        Pos::new(StoryRef::Body, self.path.clone(), self.end)
    }
    pub fn overlaps(&self, o: &Span) -> bool {
        self.path == o.path && self.start < o.end && o.start < self.end
    }
}

/// The body's paragraphs in document order (cached per call site; cheap for documents of normal size).
pub fn paragraphs(doc: &Document) -> Vec<Path> {
    doc.para_paths(StoryRef::Body)
}

/// Text of the paragraph at `path` (with U+FFFC for inline objects), if there is one.
pub fn para_text<'a>(doc: &'a Document, path: &Path) -> Option<&'a str> {
    doc.para(StoryRef::Body, path).map(|p| p.text.as_str())
}

/// Most paragraphs searched on either side when a quote has moved.
const NEAR: usize = 400;
/// The words used when a quote was edited inside: this many bytes from each end.
const EDGE: usize = 12;

/// Where an anchor is in `doc` now (`paras` is [`paragraphs`] of `doc`).
pub fn resolve(doc: &Document, paras: &[Path], a: &Anchor) -> Option<Span> {
    let path = Path(a.path.clone());
    let home = paras.iter().position(|p| *p == path);
    // Where it was, unchanged.
    if let (Some(i), Some(text)) = (home, para_text(doc, &path))
        && (text.get(a.start..a.end) == Some(a.quote.as_str()) || (a.quote.is_empty() && a.start <= text.len()))
    {
        let start = a.start.min(text.len());
        return Some(Span { para: i, path, start, end: a.end.clamp(start, text.len()) });
    }
    if a.quote.is_empty() {
        return home.map(|i| Span { para: i, path, start: 0, end: 0 });
    }
    // Moved: the occurrence nearest to where it was.
    let center = home.unwrap_or(0);
    let mut best: Option<(usize, usize, Span)> = None;
    let lo = center.saturating_sub(NEAR);
    let hi = center.saturating_add(NEAR).min(paras.len());
    for (i, p) in paras.iter().enumerate().take(hi).skip(lo) {
        let Some(text) = para_text(doc, p) else { continue };
        for (off, _) in text.match_indices(a.quote.as_str()).take(64) {
            let d = (i.abs_diff(center), off.abs_diff(a.start));
            if best.as_ref().is_none_or(|(bd, bo, _)| d < (*bd, *bo)) {
                best = Some((d.0, d.1, Span { para: i, path: p.clone(), start: off, end: off + a.quote.len() }));
            }
        }
    }
    if let Some((_, _, s)) = best {
        return Some(s);
    }
    // Edited inside: its first and last words, in that order, in the same paragraph.
    let i = home?;
    let text = para_text(doc, &path)?;
    let head = prefix(&a.quote, EDGE);
    let tail = suffix(&a.quote, EDGE);
    if head.len() + tail.len() > a.quote.len() || head.is_empty() {
        return None;
    }
    let start = text.match_indices(head).map(|(o, _)| o).min_by_key(|o| o.abs_diff(a.start))?;
    let after = start + head.len();
    let rel = text.get(after..)?.find(tail)?;
    let end = after + rel + tail.len();
    // An edit that more than tripled it isn't the same text any more.
    if end - start > a.quote.len().saturating_mul(3) + 32 {
        return None;
    }
    Some(Span { para: i, path, start, end })
}

fn prefix(s: &str, n: usize) -> &str {
    let mut end = n.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

fn suffix(s: &str, n: usize) -> &str {
    let mut start = s.len().saturating_sub(n);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s.get(start..).unwrap_or("")
}

/// An anchor for a range in one paragraph (offsets snapped to character boundaries).
pub fn make(doc: &Document, id: String, path: &Path, start: usize, end: usize) -> Option<Anchor> {
    let text = para_text(doc, path)?;
    let (mut s, mut e) = (start.min(end).min(text.len()), end.max(start).min(text.len()));
    while !text.is_char_boundary(s) {
        s -= 1;
    }
    while !text.is_char_boundary(e) {
        e += 1;
    }
    let quote = text.get(s..e)?.to_string();
    Some(Anchor { id, path: path.0.clone(), start: s, end: e, quote, lost: false })
}

/// Brings a stored anchor up to date with where it is now (or marks it lost).
pub fn refresh(doc: &Document, paras: &[Path], a: &mut Anchor) {
    match resolve(doc, paras, a) {
        Some(s) => {
            let quote = para_text(doc, &s.path).and_then(|t| t.get(s.start..s.end)).unwrap_or("").to_string();
            a.path = s.path.0;
            a.start = s.start;
            a.end = s.end;
            a.quote = quote;
            a.lost = false;
        }
        None => a.lost = true,
    }
}
