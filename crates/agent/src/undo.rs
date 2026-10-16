//! Undo of the person's actions on the layer, in the same ⌘Z history as their edits and in order
//! (`~/aitocraft/TOPICS.md` §4a), with no change to the engine's undo.
//!
//! WordCraft's undo restores whole snapshots of the document. A person's action on the layer is
//! therefore an ordinary changing command whose change to the document is (a) the bookmarks it
//! adds or removes and (b) a step counter kept in the document (a `passthrough` key no file format
//! writes). Undo restores the snapshot before it, with the counter one lower; the layer sees that
//! and applies the step's inverse. Redo puts the counter back and the step is applied again. The
//! agent's contributions are never steps, so they are never undone by the person's ⌘Z.

use std::sync::Arc;

use wordcraft_engine::Session;

use crate::layer::{Note, Private, QuestionStatus, TopicStatus};

/// The `passthrough` key of the step counter (bookkeeping, not a package part: never saved).
pub const MARK: &str = "wordcraft:agent.undoStep";

/// What a step did, so it can be taken back and done again.
#[derive(Clone, Debug)]
pub enum Op {
    /// A question asked (the bookmarks of a new topic come and go with the snapshot).
    Ask {
        question: String,
    },
    /// A topic's status and the agent's proposal, before and after.
    Status {
        topic: String,
        before: (TopicStatus, Option<crate::layer::Proposal>),
        after: (TopicStatus, Option<crate::layer::Proposal>),
    },
    /// A note deleted (with where it was in the list).
    DeleteNote {
        note: Note,
        index: usize,
    },
    PinNote {
        note: String,
        before: bool,
        after: bool,
    },
    /// An agent's place rejected (its bookmark goes with the snapshot).
    RejectPlace {
        place: String,
    },
    /// The client-data marks, before and after.
    Private {
        before: Vec<Private>,
        after: Vec<Private>,
    },
    /// A place the person added to a topic (bookmark only).
    AddPlace,
    /// A proposal applied or rejected (and the follow-up question or decision note it made).
    Proposal {
        topic: String,
        before: Box<crate::layer::EditProposal>,
        after: Box<crate::layer::EditProposal>,
        question: Option<String>,
        note: Option<Note>,
    },
    /// The person's edits of clients' copies and the file's data block, before and after.
    CopyEdits {
        before: Box<(Vec<crate::copyedit::Overlay>, Option<serde_json::Value>)>,
        after: Box<(Vec<crate::copyedit::Overlay>, Option<serde_json::Value>)>,
    },
    /// The document's context (the Agent tab's Context group), before and after.
    Context {
        before: Option<crate::context::Context>,
        after: Option<crate::context::Context>,
    },
}

/// The session's steps.
#[derive(Clone, Debug, Default)]
pub struct Log {
    /// The step the document is at.
    pub applied: u64,
    /// Steps by number (`applied` and below are done; above, undone and redoable).
    pub steps: Vec<(u64, Op)>,
}

fn mark(s: &Session) -> u64 {
    s.doc.passthrough.get(MARK).and_then(|b| std::str::from_utf8(b).ok()).and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

fn log_mut(s: &mut Session) -> Option<&mut Log> {
    s.ext.get_mut::<crate::State>().map(|st| &mut st.undo)
}

/// Records a step the person just did (call from inside the changing command that did it).
pub fn push(s: &mut Session, op: Op) {
    crate::ensure(s);
    let n = match log_mut(s) {
        Some(l) => {
            let n = l.applied.max(mark_of_steps(l)) + 1;
            // A new step drops the steps that were undone.
            l.steps.retain(|(k, _)| *k <= l.applied);
            l.steps.push((n, op));
            l.applied = n;
            n
        }
        None => return,
    };
    s.doc.passthrough.insert(MARK.into(), Arc::new(n.to_string().into_bytes()));
}

fn mark_of_steps(l: &Log) -> u64 {
    l.applied
}

/// After undo or redo: the layer follows the document's step counter.
pub fn sync(s: &mut Session) {
    crate::ensure(s);
    let target = mark(s);
    loop {
        let Some(l) = log_mut(s) else { return };
        if l.applied > target {
            let n = l.applied;
            let op = l.steps.iter().find(|(k, _)| *k == n).map(|(_, o)| o.clone());
            l.applied = n - 1;
            if let Some(op) = op {
                undo(s, &op);
            }
        } else if l.applied < target {
            let n = l.applied + 1;
            let op = l.steps.iter().find(|(k, _)| *k == n).map(|(_, o)| o.clone());
            l.applied = n;
            if let Some(op) = op {
                redo(s, &op);
            }
        } else {
            return;
        }
    }
}

fn undo(s: &mut Session, op: &Op) {
    match op {
        Op::Ask { question } => {
            let q = question.clone();
            crate::update(s, |l| {
                if let Some(x) = l.questions.iter_mut().find(|x| x.id == q) {
                    // Being worked on: the agent is told it was withdrawn.
                    x.withdrawn = x.status == QuestionStatus::Working;
                    x.hidden = true;
                }
            });
        }
        Op::Status { topic, before, .. } => set_status(s, topic, before),
        Op::DeleteNote { note, index } => {
            let (n, i) = (note.clone(), *index);
            crate::update(s, |l| {
                let i = i.min(l.notes.len());
                l.notes.insert(i, n);
            });
        }
        Op::PinNote { note, before, .. } => pin(s, note, *before),
        Op::RejectPlace { place } => reject(s, place, false),
        Op::Private { before, .. } => {
            let b = before.clone();
            crate::update(s, |l| l.private = b);
        }
        Op::AddPlace => {}
        Op::Context { before, .. } => set_context(s, before),
        Op::CopyEdits { before, .. } => copy_edits(s, before),
        Op::Proposal { topic, before, question, note, .. } => proposal(s, topic, before, question, note, true),
    }
}

fn redo(s: &mut Session, op: &Op) {
    match op {
        Op::Ask { question } => {
            let q = question.clone();
            crate::update(s, |l| {
                if let Some(x) = l.questions.iter_mut().find(|x| x.id == q) {
                    x.hidden = false;
                    // Not answered yet: it goes to the agent again.
                    if x.status.is_open() {
                        x.status = QuestionStatus::Pending;
                        x.withdrawn = false;
                        x.round = x.round.saturating_add(1);
                        x.asked_at = crate::now_ms();
                    }
                }
            });
        }
        Op::Status { topic, after, .. } => set_status(s, topic, after),
        Op::DeleteNote { note, .. } => {
            let id = note.id.clone();
            crate::update(s, |l| l.notes.retain(|n| n.id != id));
        }
        Op::PinNote { note, after, .. } => pin(s, note, *after),
        Op::RejectPlace { place } => reject(s, place, true),
        Op::Private { after, .. } => {
            let a = after.clone();
            crate::update(s, |l| l.private = a);
        }
        Op::AddPlace => {}
        Op::Context { after, .. } => set_context(s, after),
        Op::CopyEdits { after, .. } => copy_edits(s, after),
        Op::Proposal { topic, after, question, note, .. } => proposal(s, topic, after, question, note, false),
    }
}

fn proposal(s: &mut Session, topic: &str, e: &crate::layer::EditProposal, question: &Option<String>, note: &Option<Note>, hide: bool) {
    let (t, e, q, n) = (topic.to_string(), e.clone(), question.clone(), note.clone());
    crate::update(s, |l| {
        if let Some(x) = l.topic_mut(&t) {
            x.edits = Some(e);
        }
        if let Some(q) = &q
            && let Some(x) = l.questions.iter_mut().find(|x| &x.id == q)
        {
            x.hidden = hide;
        }
        if let Some(n) = &n {
            l.notes.retain(|x| x.id != n.id);
            if !hide {
                l.notes.push(n.clone());
            }
        }
    });
}

fn copy_edits(s: &mut Session, v: &(Vec<crate::copyedit::Overlay>, Option<serde_json::Value>)) {
    let v = v.clone();
    crate::update(s, |l| {
        l.overlays = v.0;
        l.data = v.1;
    });
}

fn set_context(s: &mut Session, c: &Option<crate::context::Context>) {
    let c = c.clone();
    crate::update(s, |l| l.context = c);
    crate::context::apply(s);
}

fn set_status(s: &mut Session, topic: &str, v: &(TopicStatus, Option<crate::layer::Proposal>)) {
    let (t, v) = (topic.to_string(), v.clone());
    crate::update(s, |l| {
        if let Some(x) = l.topic_mut(&t) {
            x.status = v.0;
            x.proposal = v.1;
        }
    });
}

fn pin(s: &mut Session, note: &str, on: bool) {
    let n = note.to_string();
    crate::update(s, |l| {
        if let Some(x) = l.notes.iter_mut().find(|x| x.id == n) {
            x.pinned = on;
        }
    });
}

fn reject(s: &mut Session, place: &str, on: bool) {
    let p = place.to_string();
    crate::update(s, |l| {
        for t in &mut l.topics {
            if let Some(x) = t.places.iter_mut().find(|x| x.id == p) {
                x.rejected = on;
            }
        }
    });
}
