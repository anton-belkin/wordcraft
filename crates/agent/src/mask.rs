//! The privacy gate: the document as the agent may see it.
//!
//! Ranges marked as client data read as `{{label}}` placeholders, inline objects as their text, and
//! every string that leaves for the agent is scrubbed of the real text of private ranges (and of
//! the CRM's values), so a client's name typed into a question doesn't leak either. The view keeps
//! a map from its offsets to the document's, so the agent can point at text it read ("this
//! paragraph, this quote") and the layer finds the real range, refusing ranges inside client data.

use serde_json::Value as Json;
use wordcraft_doc::para::OBJ;
use wordcraft_doc::{Document, Path, StoryRef};

use crate::anchor::{self, Span};
use crate::layer::Layer;

/// A piece of a masked paragraph: `m` in the masked text stands for `r` in the real one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub m: (usize, usize),
    pub r: (usize, usize),
    /// Text copied as is (offsets map one to one); otherwise a placeholder or an object's text.
    pub plain: bool,
    /// The placeholder's label, for client data.
    pub label: Option<String>,
}

/// A paragraph as the agent sees it.
#[derive(Clone, Debug)]
pub struct MPara {
    pub index: usize,
    pub path: Path,
    pub text: String,
    pub segs: Vec<Seg>,
    pub style: Option<String>,
    pub in_table: bool,
    /// Placeholders in it.
    pub private: Vec<String>,
}

impl MPara {
    /// The real offset of masked offset `m`; `None` inside a placeholder.
    pub fn to_real(&self, m: usize) -> Option<usize> {
        for s in &self.segs {
            if m < s.m.0 || m > s.m.1 {
                continue;
            }
            if s.plain {
                return Some(s.r.0 + (m - s.m.0));
            }
            if m == s.m.0 {
                return Some(s.r.0);
            }
            if m == s.m.1 {
                return Some(s.r.1);
            }
            return None;
        }
        (m == self.text.len()).then(|| self.segs.last().map_or(0, |s| s.r.1))
    }

    /// The masked offset of real offset `r` (a placeholder's start or end for an offset inside it).
    pub fn to_masked(&self, r: usize, end: bool) -> usize {
        for s in &self.segs {
            if r < s.r.0 || r > s.r.1 {
                continue;
            }
            if s.plain {
                return s.m.0 + (r - s.r.0);
            }
            if r == s.r.0 && !(end && s.r.0 == s.r.1) {
                return s.m.0;
            }
            return if end || r == s.r.1 { s.m.1 } else { s.m.0 };
        }
        self.text.len()
    }

    /// The masked text of a real range in this paragraph.
    pub fn masked_slice(&self, start: usize, end: usize) -> &str {
        let a = self.to_masked(start, false);
        let b = self.to_masked(end, true).max(a);
        self.text.get(a..b).unwrap_or("")
    }
}

/// The whole body as the agent sees it, and what must never reach the agent.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub paras: Vec<MPara>,
    /// (real text, placeholder), longest first.
    secrets: Vec<(String, String)>,
    /// Client data ranges, resolved.
    pub private: Vec<(Span, String)>,
}

/// Shortest real text scrubbed from strings (shorter ones would mangle ordinary words).
const MIN_SECRET: usize = 3;

impl View {
    /// The agent's view of `doc` with `layer`'s client data masked; `extra` are more (value,
    /// placeholder) pairs that must not leak (a CRM's values).
    pub fn new(doc: &Document, layer: &Layer, extra: &[(String, String)]) -> View {
        let paths = anchor::paragraphs(doc);
        let mut private: Vec<(Span, String)> = Vec::new();
        let mut secrets: Vec<(String, String)> = Vec::new();
        for p in &layer.private {
            let Some(a) = layer.anchor(&p.anchor) else { continue };
            let ph = placeholder(&p.label);
            // The text as stored: still a secret even when it can't be found any more.
            secrets.push((a.quote.clone(), ph.clone()));
            if let Some(span) = anchor::resolve(doc, &paths, a) {
                if let Some(t) = anchor::para_text(doc, &span.path).and_then(|t| t.get(span.start..span.end)) {
                    secrets.push((t.to_string(), ph.clone()));
                }
                private.push((span, p.label.clone()));
            }
        }
        secrets.extend(extra.iter().cloned());
        secrets.retain(|(s, _)| s.trim().chars().count() >= MIN_SECRET);
        secrets.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        secrets.dedup_by(|a, b| a.0 == b.0);
        let mut paras = Vec::with_capacity(paths.len());
        for (i, path) in paths.iter().enumerate() {
            let Some(p) = doc.para(StoryRef::Body, path) else { continue };
            let mut ranges: Vec<(usize, usize, &str)> =
                private.iter().filter(|(s, _)| s.para == i && s.end > s.start).map(|(s, l)| (s.start, s.end, l.as_str())).collect();
            ranges.sort_by_key(|r| (r.0, r.1));
            let mut b = Builder::default();
            let mut at = 0usize;
            let mut labels = Vec::new();
            for (s, e, label) in ranges {
                if s < at {
                    continue; // overlapping marks: the first one wins
                }
                b.text(&p.text, &p.objects, at, s);
                b.hidden(s, e, &placeholder(label), Some(label));
                labels.push(placeholder(label));
                at = e;
            }
            b.text(&p.text, &p.objects, at, p.text.len());
            // Strings an object shows (a field's result) may hold client data too.
            let text = scrub_with(&secrets, &b.text);
            let segs = if text == b.text { b.segs } else { vec![Seg { m: (0, text.len()), r: (0, p.text.len()), plain: false, label: None }] };
            paras.push(MPara { index: i, path: path.clone(), text, segs, style: p.props.style.clone(), in_table: path.0.len() > 1, private: labels });
        }
        View { paras, secrets, private }
    }

    /// Replaces every secret in `s` by its placeholder.
    pub fn scrub(&self, s: &str) -> String {
        scrub_with(&self.secrets, s)
    }

    /// Scrubs every string in a JSON value.
    pub fn scrub_json(&self, v: &mut Json) {
        match v {
            Json::String(s) => {
                let t = self.scrub(s);
                if t != *s {
                    *s = t;
                }
            }
            Json::Array(a) => a.iter_mut().for_each(|x| self.scrub_json(x)),
            Json::Object(o) => o.values_mut().for_each(|x| self.scrub_json(x)),
            _ => {}
        }
    }

    pub fn para(&self, i: usize) -> Option<&MPara> {
        self.paras.get(i)
    }

    /// The real range of `quote` (in masked text) in paragraph `para` (or the first paragraph
    /// that has it), its `occurrence`-th match (0-based). No quote: the whole paragraph.
    pub fn find(&self, para: Option<usize>, quote: Option<&str>, occurrence: usize) -> Result<Span, String> {
        let quote = quote.filter(|q| !q.is_empty());
        let candidates: Vec<&MPara> = match para {
            Some(i) => vec![self.para(i).ok_or_else(|| format!("no paragraph {i} (the document has {})", self.paras.len()))?],
            None => self.paras.iter().collect(),
        };
        let Some(q) = quote else {
            let p = candidates.first().filter(|_| para.is_some()).ok_or("give `paragraph`, `quote` or both")?;
            let len = p.segs.last().map_or(0, |s| s.r.1);
            return Ok(Span { para: p.index, path: p.path.clone(), start: 0, end: len });
        };
        let mut n = 0usize;
        for p in candidates {
            for (m, _) in p.text.match_indices(q) {
                if n < occurrence {
                    n += 1;
                    continue;
                }
                let (Some(start), Some(end)) = (p.to_real(m), p.to_real(m + q.len())) else {
                    return Err("that text starts or ends inside client data ({{…}}): quote whole placeholders".into());
                };
                return Ok(Span { para: p.index, path: p.path.clone(), start, end });
            }
        }
        Err(match para {
            Some(i) => format!("`{q}` is not in paragraph {i} (quote the text exactly as read_document shows it)"),
            None => format!("`{q}` is not in the document (quote the text exactly as read_document shows it)"),
        })
    }

    /// Whether a real range touches client data (a caret strictly inside a private range counts).
    pub fn touches_private(&self, span: &Span) -> bool {
        self.private.iter().any(|(p, _)| p.path == span.path && span.start < p.end && p.start < span.end.max(span.start + 1))
    }
}

/// `{{label}}`.
pub fn placeholder(label: &str) -> String {
    format!("{{{{{label}}}}}")
}

fn scrub_with(secrets: &[(String, String)], s: &str) -> String {
    let mut out = s.to_string();
    for (real, ph) in secrets {
        if out.contains(real.as_str()) {
            out = out.replace(real.as_str(), ph);
        }
    }
    out
}

#[derive(Default)]
struct Builder {
    text: String,
    segs: Vec<Seg>,
}

impl Builder {
    /// Real text `[a, b)`, objects as their text.
    fn text(&mut self, real: &str, objects: &[wordcraft_doc::para::InlineObject], a: usize, b: usize) {
        let Some(slice) = real.get(a..b) else { return };
        // The k-th U+FFFC of the paragraph is objects[k].
        let mut k = real.get(..a).map_or(0, |s| s.chars().filter(|c| *c == OBJ).count());
        let mut run_start = a;
        for (i, c) in slice.char_indices() {
            if c != OBJ {
                continue;
            }
            let at = a + i;
            self.plain(real, run_start, at);
            let shown = objects.get(k).map(|o| o.plain_text()).unwrap_or("");
            self.hidden(at, at + OBJ.len_utf8(), shown, None);
            k += 1;
            run_start = at + OBJ.len_utf8();
        }
        self.plain(real, run_start, b);
    }

    fn plain(&mut self, real: &str, a: usize, b: usize) {
        let Some(s) = real.get(a..b).filter(|s| !s.is_empty()) else { return };
        let m = self.text.len();
        self.text.push_str(s);
        self.segs.push(Seg { m: (m, self.text.len()), r: (a, b), plain: true, label: None });
    }

    fn hidden(&mut self, a: usize, b: usize, shown: &str, label: Option<&str>) {
        let m = self.text.len();
        self.text.push_str(shown);
        self.segs.push(Seg { m: (m, self.text.len()), r: (a, b), plain: false, label: label.map(str::to_string) });
    }
}
