//! #704/#723/#823: exact ordered selector identities, including Python surrogates.

use crate::summary::PythonText;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "tl3.selectors.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorIdentity {
    pub schema: String,
    pub count: usize,
    pub sha256: String,
    pub prefix: Vec<PythonText>,
}

/// Hash compact ensure_ascii JSON, preserving Python order and duplicates.
pub fn selector_identity(_selectors: &[PythonText]) -> SelectorIdentity {
    todo!("Red: compact Python JSON identity")
}

/// A malformed identity is not promoted to an identity by ignoring extra fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Full(Vec<PythonText>),
    Identity(SelectorIdentity),
    Invalid,
}

/// #704: comparing prefixes alone never establishes exact selection equality.
pub fn selection_matches(_candidate: &Selection, _selectors: &[PythonText]) -> bool {
    todo!("Red: compare the full selector list or the complete identity")
}
