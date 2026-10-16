//! Topics in the document (`~/aitocraft/TOPICS.md`): where each topic's places are now, their
//! numbers (topic **3**, places **3.1**, **3.2**, by position in the document, like footnotes),
//! and keeping the places' bookmarks in step with the layer after every edit:
//!
//! - **copy and paste duplicates** a place: the pasted copy becomes a new place of the same topic;
//! - **cut and paste moves** it (the bookmark comes back with the text);
//! - **deleted text keeps its whereabouts**: a collapsed bookmark where it was, and the deleted
//!   text kept in the layer ("removed here: “…”"); undo of the delete brings the place back;
//! - **undo and redo**: the person's places go and come with the edits that made them; the agent's
//!   (and migrated ones) are put back from their last known position when a restored snapshot
//!   doesn't have them yet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wordcraft_doc::{Path, Pos, StoryRef};
use wordcraft_engine::Session;

use crate::anchor;
use crate::layer::{Actor, Layer, Place, Topic, bookmark_name};
use crate::places::{self, Occ};

/// A place as it is now.
#[derive(Clone, Debug, PartialEq)]
pub struct RPlace {
    pub id: String,
    /// "3" for a one-place topic, "3.2" for the second place of a topic.
    pub label: String,
    pub from: Option<Pos>,
    pub to: Option<Pos>,
    pub removed: Option<String>,
    pub reason: String,
    pub added_by: Actor,
    /// Its other repetitions in a client's copy (a place inside a template loop shows once per item).
    pub also: Vec<(Pos, Pos)>,
}

impl RPlace {
    /// Every range it covers on screen: itself, then its repetitions.
    pub fn ranges(&self) -> impl Iterator<Item = (&Pos, &Pos)> {
        self.from.iter().zip(self.to.iter()).chain(self.also.iter().map(|(a, b)| (a, b)))
    }
}

/// A visible topic with its number and places.
#[derive(Clone, Debug, PartialEq)]
pub struct RTopic {
    pub id: String,
    pub number: usize,
    pub places: Vec<RPlace>,
}

impl RTopic {
    /// Its first place's paragraph (for the agent).
    pub fn first(&self) -> Option<&RPlace> {
        self.places.first()
    }
}

/// Where a place's text is now: its bookmark, else its last known position.
fn range_of(
    doc: &wordcraft_doc::Document,
    m: &std::collections::BTreeMap<String, Vec<Occ>>,
    paras: &[Path],
    l: &Layer,
    p: &Place,
) -> Option<(Pos, Pos)> {
    if let Some(v) = m.get(&bookmark_name(&p.id)) {
        let pick = v.iter().find(|o| o.whole() && !o.collapsed()).or_else(|| v.iter().find(|o| o.whole()));
        if let Some(o) = pick
            && let (Some(a), Some(b)) = (o.from(), o.to())
        {
            return Some(if a <= b { (a, b) } else { (b.clone(), b) });
        }
    }
    let a = l.anchor(&p.anchor)?;
    let sp = anchor::resolve(doc, paras, a)?;
    Some((sp.start_pos(), sp.end_pos()))
}

type Cache = Option<((u64, u64, u64, u64), Arc<Vec<RTopic>>)>;
static CACHE: Mutex<Cache> = Mutex::new(None);

/// The visible topics in document order, numbered, with their places in order.
pub fn resolve(s: &Session) -> Arc<Vec<RTopic>> {
    let l = crate::layer(s);
    let uid = crate::session_uid(s);
    let key = (uid, s.document_id(), s.rev(), l.revision);
    // A session that hasn't loaded its layer yet has no number: not cached.
    if uid == 0 {
        return Arc::new(compute(&s.doc, &l));
    }
    if let Ok(g) = CACHE.lock()
        && let Some((k, v)) = g.as_ref()
        && *k == key
    {
        return v.clone();
    }
    let v = Arc::new(compute(&s.doc, &l));
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((key, v.clone()));
    }
    v
}

fn compute(doc: &wordcraft_doc::Document, l: &Layer) -> Vec<RTopic> {
    if l.topics.is_empty() {
        return Vec::new();
    }
    let m = places::scan(doc);
    let paras = anchor::paragraphs(doc);
    let mut out: Vec<(Option<Pos>, &Topic, Vec<RPlace>)> = Vec::new();
    for t in l.topics.iter().filter(|t| t.visible()) {
        let mut ps: Vec<RPlace> = t
            .live_places()
            .map(|p| {
                let r = range_of(doc, &m, &paras, l, p);
                let also = m
                    .get(&crate::layer::bookmark_name(&p.id))
                    .map(|occ| {
                        occ.iter().filter_map(|o| Some((o.from()?, o.to()?))).filter(|(a, b)| a < b && r.as_ref().is_none_or(|x| x.0 != *a)).collect()
                    })
                    .unwrap_or_default();
                RPlace {
                    also,
                    id: p.id.clone(),
                    label: String::new(),
                    from: r.as_ref().map(|x| x.0.clone()),
                    to: r.map(|x| x.1),
                    removed: p.removed.clone(),
                    reason: p.reason.clone(),
                    added_by: p.added_by,
                }
            })
            .collect();
        ps.sort_by(|a, b| match (&a.from, &b.from) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let first = ps.iter().find_map(|p| p.from.clone());
        out.push((first, t, ps));
    }
    // Like footnotes: by the position of the first place; topics with no place last.
    out.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y).then(a.1.created_at.cmp(&b.1.created_at)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.created_at.cmp(&b.1.created_at),
    });
    out.into_iter()
        .enumerate()
        .map(|(i, (_, t, mut ps))| {
            let n = i + 1;
            let many = ps.len() > 1;
            for (k, p) in ps.iter_mut().enumerate() {
                p.label = if many { format!("{n}.{}", k + 1) } else { n.to_string() };
            }
            RTopic { id: t.id.clone(), number: n, places: ps }
        })
        .collect()
}

/// A topic's number now.
pub fn number(s: &Session, topic: &str) -> Option<usize> {
    resolve(s).iter().find(|t| t.id == topic).map(|t| t.number)
}

/// A place's label now ("3.2").
pub fn label(s: &Session, place: &str) -> Option<String> {
    resolve(s).iter().find_map(|t| t.places.iter().find(|p| p.id == place).map(|p| p.label.clone()))
}

/// The topic with this number, or this id.
pub fn find(s: &Session, key: &str) -> Option<String> {
    let k = key.trim().trim_start_matches("topic").trim();
    if let Ok(n) = k.parse::<usize>() {
        return resolve(s).iter().find(|t| t.number == n).map(|t| t.id.clone());
    }
    crate::layer(s).topic(k).map(|t| t.id.clone())
}

/// The place with this label ("3.2", "3") or id.
pub fn find_place(s: &Session, key: &str) -> Option<String> {
    let k = key.trim();
    let all = resolve(s);
    if let Some(p) = all.iter().flat_map(|t| t.places.iter()).find(|p| p.label == k) {
        return Some(p.id.clone());
    }
    crate::layer(s).place(k).map(|(_, p)| p.id.clone())
}

/// The visible topics whose places touch `[a, b]` (a caret counts when it is inside or at an edge).
pub fn touching(s: &Session, a: &Pos, b: &Pos) -> Vec<RTopic> {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    resolve(s).iter().filter(|t| t.places.iter().any(|p| p.ranges().any(|(x, y)| x <= b && a <= y))).cloned().collect()
}

// ---------------------------------------------------------------- keeping bookmarks in step

/// What the places were just before a command (to keep deleted text, and to tell a pasted copy).
#[derive(Clone, Debug, Default)]
pub struct Before {
    /// Place id → its text then.
    pub texts: HashMap<String, String>,
    /// Where the selection started (a paste lands there).
    pub paste_from: Option<Pos>,
    /// The selection, ordered.
    pub sel: Option<(Pos, Pos)>,
    /// Place id → where its text was.
    pub ranges: HashMap<String, (Pos, Pos)>,
}

/// Records the places before a command.
pub fn before(s: &Session) -> Before {
    let l = crate::layer(s);
    if l.topics.is_empty() {
        return Before::default();
    }
    let m = places::scan(&s.doc);
    let mut texts = HashMap::new();
    let mut ranges = HashMap::new();
    for t in &l.topics {
        for p in &t.places {
            if let Some(o) = m.get(&bookmark_name(&p.id)).and_then(|v| v.iter().find(|o| o.whole() && !o.collapsed()))
                && let (Some(a), Some(b)) = (o.from(), o.to())
            {
                texts.insert(p.id.clone(), places::text(&s.doc, &a, &b));
                ranges.insert(p.id.clone(), (a, b));
            }
        }
    }
    Before { texts, paste_from: Some(s.sel.ordered().0), sel: Some(s.sel.ordered()), ranges }
}

/// What reconciling changed in the layer.
#[derive(Default)]
struct Changes {
    places: HashMap<String, (Option<Option<String>>, Option<bool>)>,
    new_places: Vec<(String, Place, (Pos, Pos))>,
}

/// A replacement over a place (its text selected, new text typed or inserted in its stead): the
/// place spans the new text, instead of staying behind as a removed place beside it.
fn respan(s: &mut Session, place: &str, before: &Before, caret: &Pos) -> bool {
    let (Some((sa, sb)), Some((pa, pb))) = (&before.sel, before.ranges.get(place)) else { return false };
    // The selection covered (some of) the place, and text went in where it was.
    if sa == sb || !(sa <= pb && pa <= sb) || caret.path != sa.path || caret <= sa || caret.story != sa.story {
        return false;
    }
    let name = bookmark_name(place);
    let (a, b) = (crate::text_offset(&s.doc, sa), crate::text_offset(&s.doc, caret));
    for o in places::scan(&s.doc).get(&name).cloned().unwrap_or_default().iter().rev() {
        places::remove(&mut s.doc, o);
    }
    let x = s.doc.clamp(&crate::from_text_offset(&s.doc, &a, true));
    let y = s.doc.clamp(&crate::from_text_offset(&s.doc, &b, false));
    places::insert(&mut s.doc, &name, &x, &y).is_ok()
}

/// Brings the places' bookmarks and the layer in step after a command. `restore`: the command put
/// back an earlier state of the document (undo, redo).
pub fn reconcile(s: &mut Session, restore: bool, before: &Before) {
    if crate::render_state(s).is_some() {
        return;
    }
    let l = crate::layer(s);
    if l.topics.is_empty() {
        return;
    }
    let mut ch = Changes::default();
    let mut doc_changed = false;
    let caret = s.sel.focus.clone();
    let paras = anchor::paragraphs(&s.doc);
    for t in &l.topics {
        for p in &t.places {
            let name = bookmark_name(&p.id);
            let m = places::scan(&s.doc);
            let occ: Vec<Occ> = m.get(&name).cloned().unwrap_or_default();
            if p.rejected {
                for o in occ.iter().rev() {
                    places::remove(&mut s.doc, o);
                    doc_changed = true;
                }
                continue;
            }
            // A marker whose partner went with a partial delete: the place now starts (or ends) where
            // the delete was.
            if let Some(o) = occ.iter().find(|o| !o.whole()) {
                let o = o.clone();
                places::remove(&mut s.doc, &o);
                let (a, b) = match (&o.start, &o.end) {
                    (Some(st), None) if caret >= *st => (st.clone(), caret.clone()),
                    // Its start went with a replacement: from where the new text begins.
                    (None, Some(e)) if before.sel.as_ref().is_some_and(|(sa, _)| sa <= e && sa.path == e.path) => {
                        (before.sel.as_ref().map_or_else(|| caret.clone(), |(sa, _)| sa.clone()), e.clone())
                    }
                    (None, Some(e)) if caret <= *e => (caret.clone(), e.clone()),
                    (Some(x), None) | (None, Some(x)) => (x.clone(), x.clone()),
                    _ => continue,
                };
                let (a, b) = (s.doc.clamp(&a), s.doc.clamp(&b));
                let _ = places::insert(&mut s.doc, &name, &a, &b);
                doc_changed = true;
                continue;
            }
            let full: Vec<&Occ> = occ.iter().filter(|o| !o.collapsed()).collect();
            let collapsed: Vec<&Occ> = occ.iter().filter(|o| o.collapsed()).collect();
            if !full.is_empty() {
                // Copy and paste: the copy inside the pasted text becomes a new place of the topic.
                let pasted = |o: &&Occ| before.paste_from.as_ref().is_some_and(|f| o.from().is_some_and(|x| x >= *f && x <= caret));
                let keep = full.iter().position(|o| !pasted(o)).unwrap_or(0);
                let extras: Vec<Occ> = full.iter().enumerate().filter(|(i, _)| *i != keep).map(|(_, o)| (*o).clone()).collect();
                for c in collapsed.iter().rev() {
                    places::remove(&mut s.doc, c);
                    doc_changed = true;
                }
                for e in extras.iter().rev() {
                    let id = crate::update(s, |l| l.fresh_id("p"));
                    places::rename(&mut s.doc, e, &bookmark_name(&id));
                    doc_changed = true;
                    if let (Some(a), Some(b)) = (e.from(), e.to()) {
                        let place = Place { id, added_by: Actor::Human, reason: "a copy of the same text".into(), ..Default::default() };
                        ch.new_places.push((t.id.clone(), place, (a, b)));
                    }
                }
                ch.places.insert(p.id.clone(), (Some(None), Some(false)));
            } else if !collapsed.is_empty() {
                if !restore && respan(s, &p.id, before, &caret) {
                    doc_changed = true;
                    ch.places.insert(p.id.clone(), (Some(None), Some(false)));
                    continue;
                }
                for c in collapsed.iter().skip(1).rev() {
                    places::remove(&mut s.doc, c);
                    doc_changed = true;
                }
                let text = p.removed.clone().or_else(|| before.texts.get(&p.id).cloned()).unwrap_or_default();
                ch.places.insert(p.id.clone(), (Some(Some(text)), Some(false)));
            } else if restore {
                if p.restore {
                    // The agent's (or a migrated) place: back where it was last seen.
                    let r = l.anchor(&p.anchor).and_then(|a| anchor::resolve(&s.doc, &paras, a)).map(|sp| (sp.start_pos(), sp.end_pos()));
                    if let Some((a, b)) = r {
                        let _ = places::insert(&mut s.doc, &name, &a, &b);
                        doc_changed = true;
                        ch.places.insert(p.id.clone(), (None, Some(false)));
                    }
                } else {
                    ch.places.insert(p.id.clone(), (None, Some(true)));
                }
            } else if !p.hidden {
                if before.texts.contains_key(&p.id) && respan(s, &p.id, before, &caret) {
                    // Replaced by this command: it spans the new text.
                    doc_changed = true;
                    ch.places.insert(p.id.clone(), (Some(None), Some(false)));
                } else if let Some(text) = before.texts.get(&p.id) {
                    // Deleted by this command: it stays where the text was.
                    let at = s.doc.clamp(&caret);
                    let _ = places::insert(&mut s.doc, &name, &at, &at);
                    doc_changed = true;
                    ch.places.insert(p.id.clone(), (Some(Some(text.clone())), Some(false)));
                } else if p.restore {
                    // Never had a bookmark (migrated from version 1): give it one.
                    let r = l.anchor(&p.anchor).and_then(|a| anchor::resolve(&s.doc, &paras, a)).map(|sp| (sp.start_pos(), sp.end_pos()));
                    if let Some((a, b)) = r {
                        let _ = places::insert(&mut s.doc, &name, &a, &b);
                        doc_changed = true;
                    }
                }
            } else if p.hidden && !restore {
                // Hidden (its creation was undone) and still without a bookmark: stays hidden.
            }
        }
    }
    if doc_changed {
        let dirty = s.dirty;
        s.touch();
        s.dirty = dirty || !restore;
    }
    let nothing = ch.places.is_empty() && ch.new_places.is_empty();
    let doc = s.doc.clone();
    let updates = ch;
    if nothing && !doc_changed {
        return;
    }
    crate::update(s, |l| {
        for t in &mut l.topics {
            for p in &mut t.places {
                if let Some((removed, hidden)) = updates.places.get(&p.id) {
                    if let Some(r) = removed {
                        p.removed = r.clone();
                    }
                    if let Some(h) = hidden {
                        p.hidden = *h;
                    }
                }
            }
        }
        for (topic, mut place, (a, b)) in updates.new_places {
            let aid = format!("{}a", place.id);
            if let Some(an) = anchor::make(&doc, aid.clone(), &a.path, a.off, if b.path == a.path { b.off } else { usize::MAX }) {
                l.anchors.push(an);
            }
            place.anchor = aid;
            if let Some(t) = l.topic_mut(&topic) {
                t.places.push(place);
            }
        }
    });
    // Last known positions of every place with a bookmark.
    refresh_place_anchors(s);
}

/// Each place's anchor follows its bookmark (so an undo that loses the bookmark can put it back).
pub fn refresh_place_anchors(s: &mut Session) {
    let l = crate::layer(s);
    if l.topics.is_empty() {
        return;
    }
    let m = places::scan(&s.doc);
    let mut fresh = Vec::new();
    for t in &l.topics {
        for p in &t.places {
            let Some(o) = m.get(&bookmark_name(&p.id)).and_then(|v| v.iter().find(|o| o.whole() && !o.collapsed())) else { continue };
            let (Some(a), Some(b)) = (o.from(), o.to()) else { continue };
            let end = if b.path == a.path { b.off } else { anchor::para_text(&s.doc, &a.path).map_or(a.off, str::len) };
            if let Some(an) = anchor::make(&s.doc, p.anchor.clone(), &a.path, a.off, end)
                && l.anchor(&p.anchor) != Some(&an)
            {
                fresh.push(an);
            }
        }
    }
    if fresh.is_empty() {
        return;
    }
    let dirty = s.dirty;
    crate::update(s, |l| {
        for an in fresh {
            match l.anchors.iter_mut().find(|x| x.id == an.id) {
                Some(x) => *x = an,
                None => l.anchors.push(an),
            }
        }
    });
    s.dirty = dirty;
}

/// Adds a place for `[a, b)` to `topic` (a bookmark and its anchor). Returns the place id.
pub fn add_place(s: &mut Session, topic: &str, a: &Pos, b: &Pos, by: Actor, reason: &str) -> Result<String, String> {
    if a.story != StoryRef::Body || b.story != StoryRef::Body {
        return Err("places are in the body of the document".into());
    }
    let id = crate::update(s, |l| l.fresh_id("p"));
    let aid = format!("{id}a");
    // The reader's selection stays on the same text (the markers shift offsets after them).
    let (sa, sf) = (crate::text_offset(&s.doc, &s.sel.anchor), crate::text_offset(&s.doc, &s.sel.focus));
    let forward = s.sel.anchor <= s.sel.focus;
    places::insert(&mut s.doc, &bookmark_name(&id), a, b)?;
    let (na, nf) = (crate::from_text_offset(&s.doc, &sa, forward), crate::from_text_offset(&s.doc, &sf, !forward));
    s.sel = wordcraft_engine::Selection { anchor: s.doc.clamp(&na), focus: s.doc.clamp(&nf) };
    // The text now starts after the start marker.
    let m = places::scan(&s.doc);
    let (fa, fb) = m.get(&bookmark_name(&id)).and_then(|v| v.first()).and_then(|o| Some((o.from()?, o.to()?))).ok_or("the bookmark didn't take")?;
    let end = if fb.path == fa.path { fb.off } else { anchor::para_text(&s.doc, &fa.path).map_or(fa.off, str::len) };
    let an = anchor::make(&s.doc, aid.clone(), &fa.path, fa.off, end);
    let place = Place {
        id: id.clone(),
        anchor: aid,
        reason: reason.chars().take(500).collect(),
        added_by: by,
        restore: by == Actor::Agent,
        ..Default::default()
    };
    let topic = topic.to_string();
    crate::update(s, |l| {
        if let Some(an) = an {
            l.anchors.push(an);
        }
        if let Some(t) = l.topic_mut(&topic) {
            t.places.push(place);
        }
    });
    Ok(id)
}

/// Moves a place's bookmark to `[a, b)` (widen_place).
pub fn move_place(s: &mut Session, place: &str, a: &Pos, b: &Pos) -> Result<(), String> {
    let name = bookmark_name(place);
    let m = places::scan(&s.doc);
    for o in m.get(&name).cloned().unwrap_or_default().iter().rev() {
        places::remove(&mut s.doc, o);
    }
    places::insert(&mut s.doc, &name, a, b)?;
    s.touch();
    refresh_place_anchors(s);
    Ok(())
}

/// The text of a place now (and whether it is the removed text).
pub fn place_text(s: &Session, p: &RPlace) -> String {
    if let Some(r) = &p.removed {
        return r.clone();
    }
    match (&p.from, &p.to) {
        (Some(a), Some(b)) => places::text(&s.doc, a, b),
        _ => String::new(),
    }
}

/// The paragraph index of a position.
pub fn para_index(s: &Session, p: &Pos) -> Option<usize> {
    anchor::paragraphs(&s.doc).iter().position(|x| *x == p.path)
}

/// Copy and Cut take a place's bookmark along when the selection covers the whole place (as Word
/// does): the selection is widened over the markers. Returns the selection to put back after a copy.
pub fn widen_for_clipboard(s: &mut Session) -> Option<wordcraft_engine::Selection> {
    let l = crate::layer(s);
    if l.topics.is_empty() || s.sel.is_collapsed() {
        return None;
    }
    let (mut a, mut b) = s.sel.ordered();
    let m = places::scan(&s.doc);
    let mut widened = false;
    for t in &l.topics {
        for p in t.live_places() {
            let Some(o) = m.get(&bookmark_name(&p.id)).and_then(|v| v.iter().find(|o| o.whole() && !o.collapsed())) else { continue };
            let (Some(st), Some(en), Some(from), Some(to)) = (o.start.clone(), o.end.clone(), o.from(), o.to()) else { continue };
            if a <= from && to <= b {
                if a == from {
                    a = st;
                }
                if b == to {
                    b = Pos { off: en.off + wordcraft_doc::para::OBJ.len_utf8(), ..en };
                }
                widened = true;
            }
        }
    }
    if !widened {
        return None;
    }
    let before = s.sel.clone();
    s.sel = wordcraft_engine::Selection { anchor: a, focus: b };
    Some(before)
}
