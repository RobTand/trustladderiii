#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(verus_keep_ghost),
    derive(serde::Deserialize, schemars::JsonSchema)
)]
#[cfg_attr(not(verus_keep_ghost), serde(rename_all = "snake_case"))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub enum Role {
    Ingest,
    Audit,
    Planner,
    Maker,
    Reviewer,
    Classifier,
    Writer,
    Postmortem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub enum Prefix {
    None,
    Plain,
    Ephemeral,
}

/// #819 / #643: only ingest, audit and planner share prompt-cache identities.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        out == (role == Role::Ingest || role == Role::Audit || role == Role::Planner),
    )
)]
pub fn shared(role: Role) -> bool {
    matches!(role, Role::Ingest | Role::Audit | Role::Planner)
}

/// #819 / #643: orientation is cached only for shared roles with actual shared input.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        (out == Prefix::None) == (!(role == Role::Ingest || role == Role::Audit || role == Role::Planner) || !has_shared),
        (out == Prefix::Plain) == ((role == Role::Ingest || role == Role::Audit || role == Role::Planner) && has_shared && !claude),
        (out == Prefix::Ephemeral) == ((role == Role::Ingest || role == Role::Audit || role == Role::Planner) && has_shared && claude),
    )
)]
pub fn prefix(role: Role, has_shared: bool, claude: bool) -> Prefix {
    if !shared(role) || !has_shared {
        Prefix::None
    } else if claude {
        Prefix::Ephemeral
    } else {
        Prefix::Plain
    }
}
