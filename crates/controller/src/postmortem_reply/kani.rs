//! Standalone entrypoint, as in classifier/kani.rs: the controller's kernel
//! dependency disables serde under cfg(kani). No package or kernel change.
#![forbid(unsafe_code)]
mod decision;
mod proofs;
