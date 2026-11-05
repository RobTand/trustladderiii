use io::{Commit, Controller, put};
use serde_json::{Value, json};
use tl3_controller::{Bootstrap, Error, Repository, Result};
use tl3_controller::{executor::Observation, merge_dependencies::ExternalFact};
use tl3_github::{Actor, Comment};
use tl3_kernel::{Command, Event, NodeId, World};
use tl3_store::{Epoch, Revision, RowChange, Snapshot, WorldRow};
// Verbatim production arms retain their early returns inside this narrower seam.
#[allow(clippy::needless_return)]
mod current;
#[allow(clippy::too_many_arguments)]
mod delegates;
mod io;
mod properties;
#[allow(clippy::needless_return)]
mod reference;
