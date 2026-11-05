//! CC-T: the base and current Queue shells share one scripted store boundary.
#![allow(dead_code, unused_imports, unused_mut)] // Checked verbatim copies retain unrelated match arms.
#[path = "queue_translate_schedule/mod.rs"]
mod schedule;
pub use schedule::{Error, Result, adapters, decode_stored, findings, looks, request};
pub use tl3_controller::executor;
pub mod memory {
    pub fn redact_text(text: &str) -> String {
        text.to_owned()
    }
}
