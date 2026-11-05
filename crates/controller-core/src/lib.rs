//! The controller's functional core (ctrl-core workstream, PRD #730).
//!
//! Pure decisions extracted from the controller's I/O shells. Each module
//! takes small typed inputs parsed at the shell boundary and returns a typed
//! decision. No I/O, no clocks, no `unsafe`, no floats, no target-specific
//! code, so the Verus proofs hold on amd64 and arm64.
#![forbid(unsafe_code)]
pub mod looks;
pub mod looks_state;
pub mod publication;
pub mod translate;
pub mod worker_events;

pub mod memory_retain;
pub mod translate_worker;
