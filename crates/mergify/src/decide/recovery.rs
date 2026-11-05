//! Shared command protocol for legacy requeue and repaired-head recovery callers.
use super::enqueue::{CommandRead, Effect, Recovery};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub recovery: Option<Recovery>,
    pub first: Option<Effect>,
    pub second: Option<Effect>,
}

pub fn dispatch(_recovery: Option<Recovery>, _read: CommandRead) -> Transition {
    todo!("CC-M2 command protocol extraction")
}
