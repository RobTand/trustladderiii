//! The Mergify queue adapter's functional core (#735).
//!
//! The controller's Mergify adapter reads GitHub and Mergify, then decides
//! which queue facts to report and how each one reaches the kernel. This
//! module holds those decisions as pure functions over small typed domains,
//! so Verus can prove them. The adapter parses upstream JSON into these types
//! at its boundary and keeps every read, write and store access.
//!
//! - `check`: check runs as enums (status, conclusion, app, name, head).
//! - `observe`: which queue check describes the PR, and the queue state.
//! - `facts`: which facts a queue read yields (Q52, #447, merge evidence).
//! - `translate`: how a queue fact reaches the kernel, or only an incident.
//! - `enqueue`: enqueue acknowledgement, ordered write intents, command
//!   recovery and workflow rerun budgets. The shell retains its side files.
//!
//! The module does no I/O and uses no `std` item. It has no `unsafe`, no
//! floats and no target-specific code, so its proofs hold on amd64 and arm64.

use tl3_github::{decide_predicate, decide_proof, decide_while};
#[cfg(any(test, verus_keep_ghost))]
use tl3_github::{decide_infra, decide_report};

pub use tl3_github::checks::check;
pub mod enqueue;
pub mod facts;
pub mod observe;
pub mod translate;

#[cfg(kani)]
mod enqueue_proofs;

pub use check::{App, CheckView, Conclusion, Head, Name, Status, Time};
pub use enqueue::{EnqueueState, QueueEvent, decide};
pub use observe::QueueState;

pub use tl3_github::checks::{MERGIFY_APP_ID, MERGIFY_APP_SLUG, QUEUE_CHECK, QUEUE_RULE_CHECK};
pub const QUEUE_LABEL: &str = "tl3-queue";

pub mod completion;

pub mod recovery;
pub use tl3_github::checks::required;

pub mod heal;
pub mod plan;

#[cfg(test)]
mod observe_witness;

#[cfg(any(test, verus_keep_ghost))]
mod decide_witness;
pub mod queue_entry;
pub mod route;
