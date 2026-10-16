//! Extension points for crates built on top of the engine (an app links them in; the engine knows
//! nothing about them):
//!
//! - [`register_commands`] adds commands that behave like built-in ones: every [`Session`] made
//!   afterwards has them in its registry, so the UI, the CLI, the control channel and MCP reach
//!   them by id like any other command.
//! - [`register_observer`] is told before and after every command [`Session::run`] runs, so an
//!   extension can keep its own per-document state in step with the document (load it when a file
//!   opens, write it into the document before a save, record who changed what).
//! - [`Slots`] (`Session::ext`) holds an extension's per-session state outside the document, so
//!   undo never takes it away.
//!
//! Registration is process-wide and additive; an id already known keeps its first command.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

use crate::{CmdResult, CommandSpec, Session};

/// Told about every command [`Session::run`] runs.
pub trait Observer: Send + Sync {
    /// Before the command runs (after its `enabled` check passed). Changes made to the session
    /// here are part of the state the command starts from.
    fn before(&self, _s: &mut Session, _id: &str, _params: &Value) {}
    /// After the command ran (its undo step, if any, is recorded; a failed command has already
    /// put the document back).
    fn after(&self, _s: &mut Session, _id: &str, _params: &Value, _result: &CmdResult) {}
}

static COMMANDS: Mutex<Vec<CommandSpec>> = Mutex::new(Vec::new());
static OBSERVERS: Mutex<Vec<&'static dyn Observer>> = Mutex::new(Vec::new());

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Adds extension commands (to sessions made from now on). Ids registered before are skipped.
pub fn register_commands(specs: Vec<CommandSpec>) {
    let mut v = lock(&COMMANDS);
    for spec in specs {
        if !v.iter().any(|c| c.id == spec.id) {
            v.push(spec);
        }
    }
}

/// Adds a command observer.
pub fn register_observer(o: &'static dyn Observer) {
    lock(&OBSERVERS).push(o);
}

/// The registered extension commands.
pub fn commands() -> Vec<CommandSpec> {
    lock(&COMMANDS).clone()
}

/// Appends the extension commands whose ids the built-in ones don't already use.
pub(crate) fn extend(v: &mut Vec<CommandSpec>) {
    for c in commands() {
        if !v.iter().any(|b| b.id == c.id) {
            v.push(c);
        }
    }
}

fn observers() -> Vec<&'static dyn Observer> {
    lock(&OBSERVERS).clone()
}

pub(crate) fn before(s: &mut Session, id: &str, params: &Value) {
    for o in observers() {
        // An observer that panics is skipped; the command still runs.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| o.before(s, id, params)));
    }
}

pub(crate) fn after(s: &mut Session, id: &str, params: &Value, result: &CmdResult) {
    for o in observers() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| o.after(s, id, params, result)));
    }
}

/// Per-session state of extensions, one value per type. Not part of the document: undo, redo and
/// snapshots leave it alone.
#[derive(Default)]
pub struct Slots(HashMap<TypeId, Box<dyn Any + Send>>);

impl Slots {
    /// The value of type `T`, if one was put here.
    pub fn get<T: Any + Send>(&self) -> Option<&T> {
        self.0.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref())
    }

    /// The value of type `T`, made with `Default` the first time (`None` only if the slot can't
    /// hold a `T`, which never happens: the slot is keyed by `T`'s type).
    pub fn get_mut<T: Any + Send + Default>(&mut self) -> Option<&mut T> {
        let slot = self.0.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(T::default()));
        if !slot.is::<T>() {
            *slot = Box::new(T::default());
        }
        slot.downcast_mut()
    }

    /// Takes the value of type `T` out.
    pub fn take<T: Any + Send>(&mut self) -> Option<T> {
        self.0.remove(&TypeId::of::<T>()).and_then(|b| b.downcast().ok()).map(|b| *b)
    }
}

impl std::fmt::Debug for Slots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Slots({})", self.0.len())
    }
}
