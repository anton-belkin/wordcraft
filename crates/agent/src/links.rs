//! Links inside text (`~/aitocraft/TOPICS.md` §2, §2a): answers, notes and status text carry
//! Markdown links, `[§12](para:12)`, `[the fee clause](quote:12/twelve months)`, `[3.2](place:3.2)`,
//! `[topic 3](topic:3)`, `[the source](note:3/n2)`, `[NOAA](https://…)`, and a plain `§12` is
//! linked by itself.
//!
//! When a text is stored, every link to the document becomes a link to an anchor (`anchor:<id>`,
//! `place:<id>`, `topic:<id>`, `note:<id>`), so it follows the text through edits. When a text is
//! shown, each link is resolved again: a label that was an address (`§12`, `3.2`, `topic 3`) shows
//! the current one, so after a paragraph is inserted above, a past answer that said `§12` reads `§13`.

use serde_json::Value as Json;
use wordcraft_engine::Session;

use crate::anchor::{self, Span};
use crate::layer::Anchor;
use crate::topics;

/// A piece of a shown text.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Text(String),
    Link { label: String, target: Target },
}

/// Where a link leads now.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A passage of the document.
    Passage(Span),
    /// A topic's place.
    Place {
        topic: String,
        place: String,
    },
    Topic(String),
    Note {
        topic: String,
        note: String,
    },
    Url(String),
    /// Somewhere that isn't here any more (or another app's address).
    Gone,
}

/// Splits Markdown links out of `text`: `(before, Some((label, href)))` pieces.
fn parse(text: &str) -> Vec<(String, Option<(String, String)>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest.get(open..).and_then(|t| t.find("](")) else { break };
        let close = open + close_rel;
        let Some(end_rel) = rest.get(close + 2..).and_then(|t| t.find(')')) else { break };
        let end = close + 2 + end_rel;
        let label = rest.get(open + 1..close).unwrap_or("");
        let href = rest.get(close + 2..end).unwrap_or("");
        if label.contains('[') || href.contains(' ') && !href.starts_with("quote:") {
            // Not a link: keep the bracket as text.
            out.push((rest.get(..open + 1).unwrap_or("").to_string(), None));
            rest = rest.get(open + 1..).unwrap_or("");
            continue;
        }
        out.push((rest.get(..open).unwrap_or("").to_string(), Some((label.to_string(), href.to_string()))));
        rest = rest.get(end + 1..).unwrap_or("");
    }
    out.push((rest.to_string(), None));
    out
}

/// `§12` references in plain text: (start, end, paragraph).
fn section_refs(t: &str) -> Vec<(usize, usize, usize)> {
    let mut v = Vec::new();
    for (i, _) in t.match_indices('§') {
        let digits: String = t.get(i + '§'.len_utf8()..).unwrap_or("").chars().take_while(char::is_ascii_digit).take(6).collect();
        if let Ok(n) = digits.parse::<usize>() {
            v.push((i, i + '§'.len_utf8() + digits.len(), n));
        }
    }
    v
}

fn new_anchor(s: &mut Session, span: &Span) -> Option<String> {
    let doc = crate::agent_doc(s).clone();
    let id = crate::update(s, |l| l.fresh_id("l"));
    let a: Anchor = anchor::make(&doc, id.clone(), &span.path, span.start, span.end)?;
    crate::update(s, |l| l.anchors.push(a));
    Some(id)
}

/// The text as stored: links to the document become links to anchors.
pub fn store(s: &mut Session, text: &str) -> String {
    let view = crate::view(s);
    let mut out = String::with_capacity(text.len());
    for (before, link) in parse(text) {
        // Plain `§12` links itself.
        let mut at = 0;
        for (a, b, n) in section_refs(&before) {
            out.push_str(before.get(at..a).unwrap_or(""));
            let label = before.get(a..b).unwrap_or("");
            match view.find(Some(n), None, 0).ok().and_then(|sp| new_anchor(s, &sp)) {
                Some(id) => out.push_str(&format!("[{label}](anchor:{id})")),
                None => out.push_str(label),
            }
            at = b;
        }
        out.push_str(before.get(at..).unwrap_or(""));
        let Some((label, href)) = link else { continue };
        let stored = if let Some(n) = href.strip_prefix("para:").and_then(|n| n.trim().parse::<usize>().ok()) {
            view.find(Some(n), None, 0).ok().and_then(|sp| new_anchor(s, &sp)).map(|id| format!("anchor:{id}"))
        } else if let Some(q) = href.strip_prefix("quote:") {
            let (n, quote) = q.split_once('/').unwrap_or(("", q));
            let para = n.trim().parse::<usize>().ok();
            view.find(para, Some(quote), 0).ok().and_then(|sp| new_anchor(s, &sp)).map(|id| format!("anchor:{id}"))
        } else if let Some(k) = href.strip_prefix("place:") {
            topics::find_place(s, k).map(|p| format!("place:{p}"))
        } else if let Some(k) = href.strip_prefix("topic:") {
            topics::find(s, k).map(|t| format!("topic:{t}"))
        } else if let Some(k) = href.strip_prefix("note:") {
            let note = k.rsplit('/').next().unwrap_or(k);
            crate::layer(s).notes.iter().find(|x| x.id == note).map(|x| format!("note:{}", x.id))
        } else {
            None
        };
        out.push_str(&format!("[{label}]({})", stored.unwrap_or(href)));
    }
    out
}

/// A label that was an address shows the current one.
fn relabel(label: &str, now: &str) -> String {
    let l = label.trim();
    let is_section = l.starts_with('§') && l.get('§'.len_utf8()..).is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
    let is_number = !l.is_empty() && l.chars().all(|c| c.is_ascii_digit() || c == '.');
    let is_topic = l.strip_prefix("topic ").is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()));
    if is_section || is_number || is_topic { now.to_string() } else { label.to_string() }
}

/// The stored text as it is to be shown now.
pub fn render(s: &Session, text: &str) -> Vec<Seg> {
    let l = crate::layer(s);
    let doc = crate::agent_doc(s);
    let paras = anchor::paragraphs(doc);
    let mut out = Vec::new();
    for (before, link) in parse(text) {
        if !before.is_empty() {
            out.push(Seg::Text(before));
        }
        let Some((label, href)) = link else { continue };
        let (target, now) = if let Some(id) = href.strip_prefix("anchor:") {
            match l.anchor(id).and_then(|a| anchor::resolve(doc, &paras, a)) {
                Some(sp) => {
                    let now = format!("§{}", sp.para);
                    (Target::Passage(sp), now)
                }
                None => (Target::Gone, label.clone()),
            }
        } else if let Some(id) = href.strip_prefix("place:") {
            match (l.place(id), topics::label(s, id)) {
                (Some((t, _)), Some(lab)) => (Target::Place { topic: t.id.clone(), place: id.to_string() }, lab),
                _ => (Target::Gone, label.clone()),
            }
        } else if let Some(id) = href.strip_prefix("topic:") {
            match topics::number(s, id) {
                Some(n) => (Target::Topic(id.to_string()), format!("topic {n}")),
                None => (Target::Gone, label.clone()),
            }
        } else if let Some(id) = href.strip_prefix("note:") {
            match l.notes.iter().find(|n| n.id == id) {
                Some(n) => (Target::Note { topic: n.topic.clone(), note: n.id.clone() }, label.clone()),
                None => (Target::Gone, label.clone()),
            }
        } else if href.starts_with("http://") || href.starts_with("https://") {
            (Target::Url(href.clone()), label.clone())
        } else {
            (Target::Gone, label.clone())
        };
        let label = if matches!(target, Target::Note { .. } | Target::Url(_) | Target::Gone) { label } else { relabel(&label, &now) };
        out.push(Seg::Link { label, target });
    }
    out
}

/// The text with current labels, its links in the agent's terms (`para:13`, `place:3.2`,
/// `topic:3`, `note:<id>`).
pub fn for_agent(s: &Session, text: &str) -> String {
    let mut out = String::new();
    for seg in render(s, text) {
        match seg {
            Seg::Text(t) => out.push_str(&t),
            Seg::Link { label, target } => {
                let href = match &target {
                    Target::Passage(sp) => format!("para:{}", sp.para),
                    Target::Place { place, .. } => format!("place:{}", topics::label(s, place).unwrap_or_default()),
                    Target::Topic(t) => format!("topic:{}", topics::number(s, t).unwrap_or(0)),
                    Target::Note { note, .. } => format!("note:{note}"),
                    Target::Url(u) => u.clone(),
                    Target::Gone => {
                        out.push_str(&label);
                        continue;
                    }
                };
                out.push_str(&format!("[{label}]({href})"));
            }
        }
    }
    out
}

/// The plain text (labels only), for one-line previews.
pub fn plain(s: &Session, text: &str) -> String {
    render(s, text)
        .into_iter()
        .map(|seg| match seg {
            Seg::Text(t) => t,
            Seg::Link { label, .. } => label,
        })
        .collect()
}

/// `links` (the old parameter) as a sentence of links appended to the text.
pub fn append_links(text: &str, links: &Json) -> String {
    let list: Vec<String> = links
        .as_array()
        .into_iter()
        .flatten()
        .take(50)
        .filter_map(|l| {
            let label = l.get("label").and_then(Json::as_str)?;
            let href = l.get("href").and_then(Json::as_str)?;
            (!href.is_empty()).then(|| format!("[{}]({})", label.replace(['[', ']'], ""), href))
        })
        .collect();
    if list.is_empty() { text.to_string() } else { format!("{text}\n\nSee {}.", list.join(", ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_links_and_section_references_are_found() {
        let p = parse("See [the fee](quote:12/payable within 30 days) and [NOAA](https://noaa.gov).");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].1.as_ref().unwrap().1, "quote:12/payable within 30 days");
        assert_eq!(p[1].1.as_ref().unwrap(), &("NOAA".to_string(), "https://noaa.gov".to_string()));
        assert_eq!(section_refs("as §12 and §3 say"), vec![(3, 7, 12), (12, 15, 3)]);
        assert_eq!(relabel("§12", "§13"), "§13");
        assert_eq!(relabel("the clause", "§13"), "the clause");
        assert_eq!(relabel("3.2", "4.2"), "4.2");
    }
}
