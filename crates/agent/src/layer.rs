//! The agent layer of one document: what the agent and the reader added on top of the text.
//!
//! It lives in the session's extension state while the document is open (so undoing an edit never
//! takes a question or an answer away) and is written into the document as a custom XML part just
//! before every save. WordCraft keeps custom XML parts on open and save, and so does Word, so the
//! layer travels with the `.docx`. On open it is read back from that part.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use wordcraft_doc::Document;

/// Namespace of the custom XML part (and how we recognise it, whatever its part name).
pub const NS: &str = "urn:wordcraft:agent-layer:v1";
/// The part the layer is written to.
pub const PART_NAME: &str = "customXml/wordcraftAgent.xml";
/// `Document::passthrough` bookkeeping keys of `wordcraft-docx` for custom XML parts (the roots to
/// relate and the parts' content types). Pinned by the round-trip test in `tests.rs`.
const CUSTOM_XML_ROOTS: &str = "wordcraft:customXml.roots";
const EMBEDDED_PARTS: &str = "wordcraft:embedded.parts";
const REL_CUSTOM_XML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
/// Largest layer read back from a file.
const MAX_PART: usize = 32 * 1024 * 1024;

/// Who did something.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    #[default]
    Human,
    Agent,
}

/// A text range the layer points at, anchored by its text: the paragraph it was last seen in,
/// the byte offsets there and the quoted text. After edits it is found again by the quote (in
/// the same paragraph first, then nearby), so it needs no marks in the document itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub id: String,
    /// Path of the paragraph in the body (`[block]`, or `[table, row, cell, block]`…).
    pub path: Vec<u32>,
    pub start: usize,
    pub end: usize,
    /// The anchored text as it was (empty for a caret on an empty paragraph).
    #[serde(default)]
    pub quote: String,
    /// The text can't be found any more.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lost: bool,
}

/// A link in a note or an answer: a web page (`https://…`), a paragraph of the document
/// (`para:12`) or another note (`note:n3`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub href: String,
    /// A short summary of what is behind the link (for sources).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionStatus {
    #[default]
    Pending,
    /// An agent has taken it.
    Working,
    Answered,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default)]
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub anchor: String,
    pub text: String,
    #[serde(default)]
    pub status: QuestionStatus,
    #[serde(default)]
    pub asked_at: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub asked_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    /// The reader has opened the answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen: bool,
}

/// An explanation the agent left on a range: why it is written this way, a prepared answer to a
/// likely question, an assumption or a source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub anchor: String,
    /// `explanation`, `faq`, `assumption` or `source`.
    #[serde(default = "explanation")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default)]
    pub author: Actor,
    #[serde(default)]
    pub at: u64,
}

fn explanation() -> String {
    "explanation".into()
}

/// A range of client data: the agent sees `{{label}}` there instead of the text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Private {
    pub anchor: String,
    /// The placeholder's name (`client.name`), the same for the same text.
    pub label: String,
}

/// One entry of the decision history / audit trail.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    pub at: u64,
    pub actor: Actor,
    /// `decision` (why the document is the way it is) or `change` (a command that changed it).
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// What the change is based on (a source, a question id…).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Who, by name (the document's author name for people).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub who: String,
}

/// Most log entries kept (oldest dropped first).
pub const MAX_LOG: usize = 5_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub notes: Vec<Note>,
    /// Ranges holding client data.
    #[serde(default)]
    pub private: Vec<Private>,
    #[serde(default)]
    pub log: Vec<LogEntry>,
    /// Bumped on every change (not saved; lets views cache).
    #[serde(skip)]
    pub revision: u64,
}

impl Layer {
    pub fn fresh_id(&mut self, prefix: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("{prefix}{}", self.next_id)
    }

    pub fn anchor(&self, id: &str) -> Option<&Anchor> {
        self.anchors.iter().find(|a| a.id == id)
    }

    /// Adds an entry to the history. A person's edits in a row on the same paragraph with the
    /// same command (typing) are one entry, as long as they come within a minute.
    pub fn log(&mut self, entry: LogEntry) {
        if entry.kind == "change"
            && let Some(last) = self.log.last_mut()
            && last.kind == "change"
            && last.actor == entry.actor
            && last.command == entry.command
            && last.text == entry.text
            && last.source == entry.source
            && entry.at.saturating_sub(last.at) < 60_000
        {
            last.at = entry.at;
            return;
        }
        let seq = self.log.last().map_or(1, |e| e.seq.saturating_add(1));
        self.log.push(LogEntry { seq, ..entry });
        if self.log.len() > MAX_LOG {
            let extra = self.log.len() - MAX_LOG;
            self.log.drain(..extra);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty() && self.log.is_empty()
    }

    pub fn is_private_anchor(&self, id: &str) -> bool {
        self.private.iter().any(|p| p.anchor == id)
    }
}

// ---------------------------------------------------------------- in the file

/// The layer kept in a document's custom XML part, or an empty one.
pub fn read_from(doc: &Document) -> Layer {
    for bytes in doc.passthrough.values() {
        if bytes.len() > MAX_PART {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else { continue };
        if !text.contains(NS) {
            continue;
        }
        if let Some(json) = between(text, "<![CDATA[", "]]>")
            && let Ok(layer) = serde_json::from_str::<Layer>(json)
        {
            return layer;
        }
    }
    Layer::default()
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s.get(start..)?.rfind(close)? + start;
    s.get(start..end)
}

/// Writes the layer into the document's kept parts (replacing an earlier copy). An empty layer
/// removes the part.
pub fn write_into(doc: &mut Document, layer: &Layer) {
    // Earlier copies, under whatever name a save gave them.
    let ours: Vec<String> = doc
        .passthrough
        .iter()
        .filter(|(k, v)| !k.starts_with("wordcraft:") && std::str::from_utf8(v).is_ok_and(|t| t.contains(NS)))
        .map(|(k, _)| k.clone())
        .collect();
    for k in &ours {
        doc.passthrough.remove(k);
    }
    let is_ours = |part: &str| part == PART_NAME || ours.iter().any(|o| o == part);
    let roots: Vec<String> = text_lines(doc, CUSTOM_XML_ROOTS).into_iter().filter(|l| l.split('\t').nth(2).is_none_or(|p| !is_ours(p))).collect();
    let parts: Vec<String> = text_lines(doc, EMBEDDED_PARTS).into_iter().filter(|l| l.split('\t').nth(1).is_none_or(|p| !is_ours(p))).collect();
    let (mut roots, mut parts) = (roots, parts);
    if !layer.is_empty() {
        let json = serde_json::to_string(layer).unwrap_or_default();
        // CDATA can't hold `]]>`; JSON only has it inside strings, where an escaped `]` means the same.
        let json = json.replace("]]>", "\\u005d\\u005d>");
        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<agentLayer xmlns=\"{NS}\"><![CDATA[{json}]]></agentLayer>");
        doc.passthrough.insert(PART_NAME.into(), Arc::new(xml.into_bytes()));
        roots.push(format!("D\t{REL_CUSTOM_XML}\t{PART_NAME}"));
        parts.insert(0, format!("P\t{PART_NAME}\tapplication/xml"));
    }
    set_lines(doc, CUSTOM_XML_ROOTS, &roots);
    set_lines(doc, EMBEDDED_PARTS, &parts);
}

fn text_lines(doc: &Document, key: &str) -> Vec<String> {
    doc.passthrough.get(key).map(|b| String::from_utf8_lossy(b).lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn set_lines(doc: &mut Document, key: &str, lines: &[String]) {
    if lines.is_empty() {
        doc.passthrough.remove(key);
    } else {
        let mut s = lines.join("\n");
        s.push('\n');
        doc.passthrough.insert(key.into(), Arc::new(s.into_bytes()));
    }
}
