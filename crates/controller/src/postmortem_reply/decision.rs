//! Finite facts for Q32 postmortem recommendations, parsed at the JSON boundary.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const MAX_RECOMMENDATIONS: usize = 3;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Operation {
    Add,
    Remove,
    Replace,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Recommendation {
    pub operation: Operation,
    pub before_nonempty: bool,
    pub after_nonempty: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Reply {
    TooMany,
    Recommendations([Option<Recommendation>; MAX_RECOMMENDATIONS]),
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    TooMany,
    OperationText,
}

impl Violation {
    pub fn message(self) -> &'static str {
        match self {
            Self::TooMany => "at most three recommendations",
            Self::OperationText => "a recommendation's before and after must fit its operation",
        }
    }
}

// Both builds execute the same traversal; only its proof contract is erased.
#[cfg(not(verus_keep_ghost))]
macro_rules! recommendation_loop {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! recommendation_loop {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

/// Q32 / #403: refuse excess recommendations before checking their operations.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == expected(reply),
))]
pub fn decide(reply: Reply) -> Result<(), Violation> {
    let Reply::Recommendations(items) = reply else {
        return Err(Violation::TooMany);
    };
    let mut index = 0;
    recommendation_loop! { (index < MAX_RECOMMENDATIONS) [
        invariant index <= MAX_RECOMMENDATIONS,
            reply == Reply::Recommendations(items),
            forall|j: int| 0 <= j < index ==> valid_slot(items[j]),
        decreases MAX_RECOMMENDATIONS - index,
    ] {
        let item = items[index];
        index += 1;
        let Some(item) = item else { continue };
        let valid = match item.operation {
            Operation::Add => !item.before_nonempty && item.after_nonempty,
            Operation::Remove => item.before_nonempty && !item.after_nonempty,
            Operation::Replace => item.before_nonempty && item.after_nonempty,
        };
        if !valid {
            #[cfg(verus_keep_ghost)]
            vstd::prelude::verus_exec_expr! { proof {
                assert(!valid_slot(items[index as int - 1]));
            } }
            return Err(Violation::OperationText);
        }
    } }
    Ok(())
}

#[cfg(verus_keep_ghost)]
verus! {
/// Q32/#403: independent operation/text truth table, including vacant slots.
pub open spec fn valid_slot(item: Option<Recommendation>) -> bool {
    match item {
        None => true,
        Some(r) => matches!((r.operation, r.before_nonempty, r.after_nonempty),
            (Operation::Add, false, true) | (Operation::Remove, true, false) | (Operation::Replace, true, true)),
    }
}
/// Q32/#403: retain the exact three-slot parsed domain and count precedence.
pub open spec fn expected(reply: Reply) -> Result<(), Violation> {
    match reply {
        Reply::TooMany => Err(Violation::TooMany),
        Reply::Recommendations(items) => if valid_slot(items[0]) && valid_slot(items[1]) && valid_slot(items[2]) {
            Ok(())
        } else { Err(Violation::OperationText) },
    }
}
}
