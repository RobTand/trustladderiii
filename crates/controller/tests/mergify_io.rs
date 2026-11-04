//! #751/#760: extraction equivalence under changing I/O and crashes.
#[path = "mergify_io/mod.rs"]
mod mergify_io;
pub use mergify_io::{Error, Result, decode_stored, executor};
