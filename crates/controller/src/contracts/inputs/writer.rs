use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Question,
    Clarification,
    Note,
    Summary,
}

/// The request a draft is written for. It names the writer run's operation and
/// trace tags like any other worker run's (#427); it isn't writer-input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteFor {
    pub node: u64,
    pub generation: u64,
    /// `owner/name`, as the other roles' traces tag it.
    pub repo: String,
    pub issue: u64,
    pub attempt: u8,
    pub round: u8,
}

/// Protocol v1 writer-input. The run key belongs to the supervisor, not role input.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteRequest {
    pub kind: MessageKind,
    pub facts: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
    /// `None` for a write that belongs to no request.
    #[serde(skip)]
    pub request: Option<WriteFor>,
}
