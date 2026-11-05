use io::{Commit, Controller, put};
use serde_json::{Value, json};
use tl3_controller::{Bootstrap, Error, Repository, Result};
use tl3_github::{Actor, Comment};
use tl3_kernel::Event;
use tl3_store::{Epoch, Revision, RowChange, Snapshot, WorldRow};
mod current;
mod io;
mod properties;
mod reference;
#[derive(Clone, Debug)]
enum Observation {
    GitHubComment {
        repository: Repository,
        issue: u64,
        comment: Comment,
    },
}
