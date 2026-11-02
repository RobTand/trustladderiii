//! #430: the worker appends each finished span to `trace.jsonl` before any
//! network call. When an attempt ends, the controller ships what the worker
//! couldn't, and closes the trace of a worker that died first.
use crate::Result;
use std::{path::Path, process::Child};

pub(super) const LOG: &str = "trace.jsonl";

/// Start the flusher for an ended attempt whose span log isn't marked shipped.
pub(super) fn start_flush(_attempt: &Path) -> Result<Option<Child>> {
    todo!("#430")
}

/// The run's ending is the trigger. Tracing never delays or fails it.
pub(super) fn flush(_attempt: &Path) {}

#[cfg(test)]
pub(super) static FLUSHES: std::sync::Mutex<Vec<(std::path::PathBuf, Child)>> =
    std::sync::Mutex::new(Vec::new());
