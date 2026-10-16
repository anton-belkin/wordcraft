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
    /// Spoken, being transcribed: not with the agent yet (kept in memory only, never saved).
    Transcribing,
    /// The transcription failed: the reader can retry or dismiss it (memory only).
    Failed,
}

impl QuestionStatus {
    /// Not a question for the agent yet: a spoken one still turning into text.
    pub fn is_draft(self) -> bool {
        matches!(self, QuestionStatus::Transcribing | QuestionStatus::Failed)
    }
    /// With the agent: waiting or taken.
    pub fn is_open(self) -> bool {
        matches!(self, QuestionStatus::Pending | QuestionStatus::Working)
    }
}

/// What the reader pointed at while speaking: marker `⟦n⟧` in the question's text, a place of
/// the question's topic.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuestionRef {
    pub n: u32,
    pub place: String,
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
    /// The topic it belongs to (v2).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub topic: String,
    /// The person took it back while the agent was on it (undo): the agent drops it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withdrawn: bool,
    /// Undone: not shown, not sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// How many times it was sent (a redo sends it again).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    /// Passages pointed at while it was spoken.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<QuestionRef>,
    /// Asked by voice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spoken: bool,
    /// Why the transcription failed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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
    /// The topic it belongs to (v2).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub topic: String,
    /// Kept in view (the person pinned it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

fn explanation() -> String {
    "explanation".into()
}

// ---------------------------------------------------------------- topics (v2)

/// A topic's status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusKind {
    /// A question or doubt, no decision yet.
    #[default]
    Open,
    /// Decided that something here must change.
    Revise,
    /// Settled and accepted (only a person sets it).
    Agreed,
    /// Nothing to decide: an explanation, a source, a caveat.
    Info,
}

impl StatusKind {
    pub const ALL: [StatusKind; 4] = [StatusKind::Open, StatusKind::Revise, StatusKind::Agreed, StatusKind::Info];
    pub fn parse(s: &str) -> Option<StatusKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Some(StatusKind::Open),
            "revise" => Some(StatusKind::Revise),
            "agreed" => Some(StatusKind::Agreed),
            "info" | "for information" | "information" => Some(StatusKind::Info),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            StatusKind::Open => "open",
            StatusKind::Revise => "revise",
            StatusKind::Agreed => "agreed",
            StatusKind::Info => "info",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            StatusKind::Open => "Open",
            StatusKind::Revise => "Revise",
            StatusKind::Agreed => "Agreed",
            StatusKind::Info => "For information",
        }
    }
}

/// A topic's status with its free text, who set it and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TopicStatus {
    #[serde(default)]
    pub kind: StatusKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default)]
    pub by: Actor,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub who: String,
    #[serde(default)]
    pub at: u64,
    /// Set by someone (not just the default Open of a first question).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub set: bool,
}

/// The agent's proposal to mark a topic Agreed, which the person confirms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub reasons: String,
    #[serde(default)]
    pub at: u64,
}

/// Where a topic applies: a bookmark `_GcPlace_<id>` in the document, with its last known
/// position and text (`anchor`) to fall back on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub id: String,
    /// Its anchor (last known paragraph, offsets and text).
    pub anchor: String,
    /// Why the agent added it ("depends on 3.1 via the fee clause").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default)]
    pub added_by: Actor,
    /// Its text was deleted: it stays where it was, with the deleted text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<String>,
    /// The person rejected it (an agent's place).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rejected: bool,
    /// Its bookmark is gone with an undo: not shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Put back from its last known position when an undo loses its bookmark (the agent's places
    /// and migrated ones; the person's own go and come with their edits).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restore: bool,
}

/// One question or theme about the document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub id: String,
    #[serde(default)]
    pub places: Vec<Place>,
    #[serde(default)]
    pub status: TopicStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
    /// The agent's proposed changes (options of recorded commands), until applied or rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<EditProposal>,
    #[serde(default)]
    pub created_by: Actor,
    #[serde(default)]
    pub created_at: u64,
}

impl Topic {
    pub fn live_places(&self) -> impl Iterator<Item = &Place> {
        self.places.iter().filter(|p| !p.rejected && !p.hidden)
    }
    /// Shown: it has a place that isn't hidden or rejected (or no places at all).
    pub fn visible(&self) -> bool {
        self.places.is_empty() || self.live_places().next().is_some()
    }
}

/// One recorded command of a proposal: what, where (in the agent's view of the draft it ran on)
/// and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub command: String,
    #[serde(default)]
    pub params: serde_json::Value,
    /// `{paragraph, quote?, occurrence?}`.
    #[serde(default)]
    pub at: serde_json::Value,
    /// The paragraph's (masked) text then, to find it again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub para_text: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// What it does, in a line.
    #[serde(default)]
    pub summary: String,
}

/// One option of a proposal (A, B…).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalOption {
    pub option: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalState {
    #[default]
    Pending,
    Applied,
    Rejected,
    /// A step no longer fits the document: the agent redoes it.
    Outdated,
}

impl ProposalState {
    pub fn id(self) -> &'static str {
        match self {
            ProposalState::Pending => "pending",
            ProposalState::Applied => "applied",
            ProposalState::Rejected => "rejected",
            ProposalState::Outdated => "outdated",
        }
    }
}

/// A topic's proposed changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditProposal {
    #[serde(default)]
    pub options: Vec<ProposalOption>,
    #[serde(default)]
    pub state: ProposalState,
    /// The option applied.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chosen: String,
    /// The person's comment on a rejection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default)]
    pub at: u64,
}

/// The bookmark name of a place.
pub fn bookmark_name(place: &str) -> String {
    format!("{BOOKMARK_PREFIX}{place}")
}

/// Bookmarks of places start with this (Word hides bookmarks that start with "_").
pub const BOOKMARK_PREFIX: &str = "_GcPlace_";

/// The layer format.
pub const VERSION: u32 = 2;

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
    /// The background agent's session for this document (resumed when it is opened again).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_session: String,
    /// The folder that session runs in (a session is resumed only there).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_cwd: String,
    /// The person's edits of clients' copies (kept per client, re-applied after each render).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<crate::copyedit::Overlay>,
    /// The file's own client data (`{"client": {...}}`): a copy rendered from it is editable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// What the agent knows: the document's context (`None`: the app's last used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<crate::context::Context>,
    /// Topics (v2): places, status, notes, discussion.
    #[serde(default)]
    pub topics: Vec<Topic>,
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
        self.anchors.is_empty() && self.log.is_empty() && self.topics.is_empty() && self.overlays.is_empty() && self.data.is_none()
    }

    pub fn topic(&self, id: &str) -> Option<&Topic> {
        self.topics.iter().find(|t| t.id == id)
    }

    pub fn topic_mut(&mut self, id: &str) -> Option<&mut Topic> {
        self.topics.iter_mut().find(|t| t.id == id)
    }

    /// The topic and place of a place id.
    pub fn place(&self, id: &str) -> Option<(&Topic, &Place)> {
        self.topics.iter().find_map(|t| t.places.iter().find(|p| p.id == id).map(|p| (t, p)))
    }

    /// Version 1 (questions and notes on anchors) to version 2 (topics): one topic per question,
    /// its anchor as the place, status Open (For information when answered and read); each note a
    /// topic of its own, For information.
    pub fn migrate(&mut self) {
        if self.version >= VERSION {
            return;
        }
        let questions = self.questions.clone();
        for (i, q) in questions.iter().enumerate() {
            if !q.topic.is_empty() {
                continue;
            }
            let t = self.fresh_id("t");
            let p = self.fresh_id("p");
            let done = q.status == QuestionStatus::Answered && q.seen;
            self.topics.push(Topic {
                id: t.clone(),
                places: vec![Place { id: p, anchor: q.anchor.clone(), restore: true, ..Default::default() }],
                status: TopicStatus { kind: if done { StatusKind::Info } else { StatusKind::Open }, at: q.asked_at, ..Default::default() },
                created_at: q.asked_at,
                ..Default::default()
            });
            if let Some(x) = self.questions.get_mut(i) {
                x.topic = t;
            }
        }
        let notes = self.notes.clone();
        for (i, n) in notes.iter().enumerate() {
            if !n.topic.is_empty() {
                continue;
            }
            let t = self.fresh_id("t");
            let p = self.fresh_id("p");
            self.topics.push(Topic {
                id: t.clone(),
                places: if n.anchor.is_empty() {
                    vec![]
                } else {
                    vec![Place { id: p, anchor: n.anchor.clone(), added_by: n.author, restore: true, ..Default::default() }]
                },
                status: TopicStatus { kind: StatusKind::Info, by: n.author, at: n.at, ..Default::default() },
                created_by: n.author,
                created_at: n.at,
                ..Default::default()
            });
            if let Some(x) = self.notes.get_mut(i) {
                x.topic = t;
            }
        }
        self.version = VERSION;
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
            && let Ok(mut layer) = serde_json::from_str::<Layer>(json)
        {
            layer.migrate();
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
    if !layer.is_empty() || layer.context.is_some() {
        let mut l = layer.clone();
        l.version = VERSION;
        // Spoken questions still being transcribed live in memory only.
        l.questions.retain(|q| !q.status.is_draft());
        let json = serde_json::to_string(&l).unwrap_or_default();
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
