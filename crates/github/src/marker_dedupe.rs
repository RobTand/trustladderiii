#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub keep: Option<usize>,
    pub delete_from: usize,
}

/// State-table delivery contract, Comment/message: retain the oldest marker.
/// Inputs are sorted by comment ID at the boundary. The bound is the complete
/// usize domain, with no bound on a trace's number of reconciliations.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        out.keep == (if copies == 0 { None } else { Some(0usize) }),
        out.delete_from == (if copies == 0 { 0usize } else { 1usize }),
        out.delete_from <= copies,
        copies > 1 ==> out.delete_from < copies,
    )
)]
pub fn plan(copies: usize) -> Plan {
    if copies == 0 {
        Plan { keep: None, delete_from: 0 }
    } else {
        Plan { keep: Some(0), delete_from: 1 }
    }
}

/// Comment delivery anti-vacuity: an absent marker has no retained index.
#[cfg(verus_keep_ghost)]
#[vstd::prelude::verus_spec(out => ensures out.keep == None, out.delete_from == 0)]
pub fn absent_witness() -> Plan {
    plan(0)
}

/// Comment delivery anti-vacuity: two visible copies retain one and delete one.
#[cfg(verus_keep_ghost)]
#[vstd::prelude::verus_spec(out => ensures out.keep == Some(0usize), out.delete_from == 1)]
pub fn duplicate_witness() -> Plan {
    plan(2)
}

#[cfg(verus_keep_ghost)]
fn main() {}
