use io::{Commit, Controller, World, put};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tl3_controller::executor::Observation;
use tl3_controller::{Error, Result};
use tl3_kernel::{Event, Fact, HeadId, NodeId, Reply};
use tl3_store::RowChange;
mod conflict_current;
mod conflict_reference;
mod io;
mod pass_current;
mod pass_properties;
mod pass_reference;
mod properties;
mod rebased_current;
mod rebased_reference;
/// Copies the committed regressions to writable test output so proptest can
/// append new minimized failures there instead of the read-only runfiles tree.
fn writable_regressions(committed: &str, name: &str) -> &'static str {
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("TEST_TMPDIR"))
        .expect("writable test output");
    let path = std::path::PathBuf::from(output).join(name);
    std::fs::write(&path, committed).unwrap();
    Box::leak(path.to_str().unwrap().to_owned().into_boxed_str())
}
