//! #821: deterministic PB evidence decisions. Files and subprocesses remain in the worker.
use serde_json::Value;

pub fn decide(_operation: &str, _document: Value) -> Value {
    todo!("PB evidence decision migration")
}
