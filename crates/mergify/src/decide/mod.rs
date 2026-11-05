//! The Mergify queue adapter's functional core (#735).
//!
//! The controller's Mergify adapter reads GitHub and Mergify, then decides
//! which queue facts to report and how each one reaches the kernel. This
//! module holds those decisions as pure functions over small typed domains,
//! so Kani can prove them. The adapter parses upstream JSON into these types
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

#[macro_use]
mod verification;

pub mod check;
pub mod delivery_step;
pub mod enqueue;
pub mod facts;
pub mod observe;
pub mod translate;

#[cfg(kani)]
mod enqueue_proofs;
#[cfg(kani)]
mod proofs;

pub use check::{App, CheckView, Conclusion, Head, Name, Status, Time};
pub use enqueue::{EnqueueState, QueueEvent, decide};
pub use observe::QueueState;

/// The check run Mergify's GitHub App posts for a queued PR.
pub const QUEUE_CHECK: &str = "Mergify Merge Queue";
/// The check run for TL3's queue rule.
pub const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";
pub const MERGIFY_APP_ID: u64 = 10562;
pub const MERGIFY_APP_SLUG: &str = "mergify";
pub const QUEUE_LABEL: &str = "tl3-queue";

pub mod completion;
#[cfg(test)]
mod observe_witness;

pub mod required;
