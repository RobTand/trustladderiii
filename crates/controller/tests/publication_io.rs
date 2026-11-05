// Keep crate:: paths in the verbatim shells bound to the same recorded boundary.
#[path = "publication_io/mod.rs"]
mod publication_io;
pub use publication_io::{
    approval_delivery, decode_stored, evidence, executor, goals, pr_body, publication_failure,
    question_findings, worker_inputs,
};
pub use tl3_controller::{Error, Repository, Result, redaction, steering};
