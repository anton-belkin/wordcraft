//! Places in the document: bookmarks named `_GcPlace_<place id>` (Word keeps bookmarks that start
//! with "_" hidden). They move with the text they mark, survive a trip through Word, and are what a
//! topic's places point at. This module reads, adds, renames and removes them; [`crate::topics`]
//! keeps them in step with the layer after every edit.

use std::collections::BTreeMap;

use wordcraft_doc::para::{InlineObject, OBJ};
use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Document, Pos, StoryRef};

use crate::layer::BOOKMARK_PREFIX;

/// One bookmark of ours: where its start and end markers are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occ {
    /// The start marker (`None` when only the end is left).
    pub start: Option<Pos>,
    /// The end marker (`None` when only the start is left).
    pub end: Option<Pos>,
}

impl Occ {
    /// Where the marked text begins (just after the start marker).
    pub fn from(&self) -> Option<Pos> {
        self.start.as_ref().map(|p| Pos { off: p.off + OBJ.len_utf8(), ..p.clone() })
    }
    /// Where it ends (at the end marker).
    pub fn to(&self) -> Option<Pos> {
        self.end.clone()
    }
    /// Start and end with nothing between: the place of deleted text.
    pub fn collapsed(&self) -> bool {
        match (self.from(), self.to()) {
            (Some(a), Some(b)) => a >= b,
            _ => false,
        }
    }
    pub fn whole(&self) -> bool {
        self.start.is_some() && self.end.is_some()
    }
}

/// Our bookmarks in the body, by name, in document order.
pub fn scan(doc: &Document) -> BTreeMap<String, Vec<Occ>> {
    let mut out: BTreeMap<String, Vec<Occ>> = BTreeMap::new();
    for path in doc.para_paths(StoryRef::Body) {
        let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
        if !p.text.contains(OBJ) {
            continue;
        }
        for off in p.object_offsets() {
            let pos = || Pos::new(StoryRef::Body, path.clone(), off);
            match p.object_at(off) {
                Some(InlineObject::BookmarkStart { name }) if name.starts_with(BOOKMARK_PREFIX) => {
                    out.entry(name.clone()).or_default().push(Occ { start: Some(pos()), end: None });
                }
                Some(InlineObject::BookmarkEnd { name }) if name.starts_with(BOOKMARK_PREFIX) => {
                    let v = out.entry(name.clone()).or_default();
                    // The end closes the earliest start still open.
                    match v.iter_mut().find(|o| o.end.is_none() && o.start.is_some()) {
                        Some(o) => o.end = Some(pos()),
                        None => v.push(Occ { start: None, end: Some(pos()) }),
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn props_at(doc: &Document, p: &Pos) -> CharProps {
    doc.para(p.story, &p.path).map(|x| x.props_at(p.off).clone()).unwrap_or_default()
}

/// Marks `[a, b)` with bookmark `name` (an empty range gives a collapsed bookmark).
pub fn insert(doc: &mut Document, name: &str, a: &Pos, b: &Pos) -> Result<(), String> {
    let (a, b) = if a <= b { (doc.clamp(a), doc.clamp(b)) } else { (doc.clamp(b), doc.clamp(a)) };
    let pa = props_at(doc, &a);
    let pb = props_at(doc, &b);
    // The end first: inserting it doesn't move `a`.
    doc.insert_object(&b, InlineObject::BookmarkEnd { name: name.to_string() }, &pb).map_err(|e| e.to_string())?;
    doc.insert_object(&a, InlineObject::BookmarkStart { name: name.to_string() }, &pa).map_err(|e| e.to_string())?;
    Ok(())
}

fn remove_marker(doc: &mut Document, p: &Pos) {
    let b = Pos { off: p.off + OBJ.len_utf8(), ..p.clone() };
    let _ = doc.delete_range(p, &b);
}

/// Removes one bookmark's markers.
pub fn remove(doc: &mut Document, o: &Occ) {
    // The end first: removing it doesn't move the start.
    if let Some(e) = &o.end {
        remove_marker(doc, e);
    }
    if let Some(s) = &o.start {
        remove_marker(doc, s);
    }
}

/// Renames one bookmark (both its markers).
pub fn rename(doc: &mut Document, o: &Occ, to: &str) {
    for (p, start) in [(o.start.as_ref(), true), (o.end.as_ref(), false)] {
        let Some(p) = p else { continue };
        if let Ok(para) = doc.para_mut(p.story, &p.path)
            && let Some(obj) = para.object_at_mut(p.off)
        {
            match obj {
                InlineObject::BookmarkStart { name } if start => *name = to.to_string(),
                InlineObject::BookmarkEnd { name } if !start => *name = to.to_string(),
                _ => {}
            }
            para.touch();
        }
    }
}

/// The plain text between `a` and `b` (objects left out).
pub fn text(doc: &Document, a: &Pos, b: &Pos) -> String {
    if a >= b {
        return String::new();
    }
    doc.copy_range(a, b).plain_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_are_added_found_renamed_and_removed() {
        let mut d = Document::from_text("Alpha beta gamma\nDelta");
        let a = Pos::body(0, 6);
        let b = Pos::body(0, 10);
        insert(&mut d, "_GcPlace_p1", &a, &b).unwrap();
        let m = scan(&d);
        let o = &m["_GcPlace_p1"][0];
        assert!(o.whole() && !o.collapsed());
        assert_eq!(text(&d, &o.from().unwrap(), &o.to().unwrap()), "beta");
        rename(&mut d, o, "_GcPlace_p2");
        let m = scan(&d);
        assert!(m.contains_key("_GcPlace_p2") && !m.contains_key("_GcPlace_p1"));
        remove(&mut d, &m["_GcPlace_p2"][0]);
        assert!(scan(&d).is_empty());
        assert_eq!(d.plain_text(StoryRef::Body), "Alpha beta gamma\nDelta");
        // Collapsed.
        insert(&mut d, "_GcPlace_p3", &Pos::body(1, 2), &Pos::body(1, 2)).unwrap();
        assert!(scan(&d)["_GcPlace_p3"][0].collapsed());
    }
}
