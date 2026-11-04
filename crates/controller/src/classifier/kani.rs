//! Standalone Kani entrypoint for the same decision code the controller uses.
//! The full controller cannot compile under cfg(kani): the kernel intentionally
//! disables serde derives in that mode. This entrypoint needs no kernel changes
//! and adds no Cargo package.
#![forbid(unsafe_code)]
mod decision;
mod proofs;
