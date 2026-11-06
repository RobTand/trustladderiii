use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tl3_controller::{Error, Result};
use tl3_kernel::{Event, Fact, NodeId, Phase, Reply, World};
use io::{Commit, Controller, RowChange, Snapshot, put};
mod contracts { pub use tl3_controller::contracts::MESSAGE_CHARS; }
#[allow(unused_imports)] // Verbatim prefix keeps the full source import list.
mod findings { include!("findings.rs"); }
macro_rules! eprintln {
    ($($arg:tt)*) => { crate::io::diagnostic(format!($($arg)*)) };
}
mod io;
mod reference;
mod current;
mod properties;
