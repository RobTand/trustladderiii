//! Standalone entrypoint imports the exact production reviewer decision core.
//! The controller's kernel dependency intentionally omits serde under cfg(kani).
#![forbid(unsafe_code)]
mod decision;
mod proofs;
