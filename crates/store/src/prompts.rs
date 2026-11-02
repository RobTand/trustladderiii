//! Role prompt versions (PRD #327, C5). tl3d seeds the built-in prompts and
//! records which version each worker run uses; the console adds versions and
//! moves each role's active pointer. None of this is world state.

use crate::{Result, Store};
use postgres::Client;

/// Who added a built-in version, in the version history.
pub const BUILTIN_AUTHOR: &str = "TL3 deploy";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptVersion {
    pub id: i64,
    /// 1 for a role's first version, counting up.
    pub number: i64,
    pub role: String,
    pub body: String,
    pub builtin: bool,
    pub author: String,
    pub note: String,
    pub created_ms: i64,
}

/// A role's versions, newest first, and which one runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolePrompts {
    pub role: String,
    pub active: i64,
    pub activated_by: String,
    pub activated_ms: i64,
    /// A built-in version a deploy shipped while another version stayed active,
    /// until Rob activates it or keeps his own.
    pub builtin_update: Option<i64>,
    pub versions: Vec<PromptVersion>,
}

impl RolePrompts {
    pub fn version(&self, id: i64) -> Option<&PromptVersion> {
        self.versions.iter().find(|version| version.id == id)
    }
}

/// Why the console's edit changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    UnknownRole,
    UnknownVersion,
    /// The edit started from a version that is no longer active.
    Stale {
        active: i64,
    },
    EmptyText,
    EmptyNote,
    /// Nothing to keep: no built-in update waits for this version.
    NoUpdate,
}

pub type Edit = std::result::Result<RolePrompts, Refusal>;

impl Store {
    pub fn seed_prompts(&mut self, _builtins: &[(&str, &str)]) -> Result<()> {
        todo!("seed built-in prompt versions")
    }

    pub fn run_prompt(&mut self, _run_key: &str, _role: &str) -> Result<Option<PromptVersion>> {
        todo!("record a run's prompt version")
    }
}

pub struct Prompts {
    #[allow(dead_code)]
    client: Client,
}

impl Prompts {
    pub fn open(_settings: &tl3_config::Store) -> Result<Self> {
        todo!("open the prompt session")
    }

    pub fn roles(&mut self) -> Result<Vec<RolePrompts>> {
        todo!()
    }

    pub fn role(&mut self, _role: &str) -> Result<Option<RolePrompts>> {
        todo!()
    }

    pub fn save(
        &mut self,
        _role: &str,
        _body: &str,
        _note: &str,
        _author: &str,
        _base: i64,
    ) -> Result<Edit> {
        todo!()
    }

    pub fn activate(&mut self, _role: &str, _version: i64, _author: &str) -> Result<Edit> {
        todo!()
    }

    pub fn keep(&mut self, _role: &str, _builtin: i64) -> Result<Edit> {
        todo!()
    }
}
