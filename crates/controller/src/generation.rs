//! Configured generation at exact committed controller boundaries.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub completed: bool,
    pub changed: Vec<String>,
    pub message: String,
    pub head: String,
}
