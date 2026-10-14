//! Line-editing state shared by the request thread, which serves the alias
//! and history APIs, and the input thread, whose cooked reads use them.
//! Locked briefly by either; never held while waiting for output.
use crate::alias::Aliases;
use crate::history::Histories;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Default)]
pub struct Editing {
    pub histories: Histories,
    pub aliases: Aliases,
}

pub type SharedEditing = Arc<Mutex<Editing>>;

pub fn lock(editing: &SharedEditing) -> MutexGuard<'_, Editing> {
    editing
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
