//! Finite facts for Q32 postmortem recommendations, parsed at the JSON boundary.

pub const MAX_RECOMMENDATIONS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Operation {
    Add,
    Remove,
    Replace,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Recommendation {
    pub operation: Operation,
    pub before_nonempty: bool,
    pub after_nonempty: bool,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Reply {
    TooMany,
    Recommendations([Option<Recommendation>; MAX_RECOMMENDATIONS]),
}

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

/// Q32 / #403: refuse excess recommendations before checking their operations.
pub fn decide(reply: Reply) -> Result<(), Violation> {
    let Reply::Recommendations(items) = reply else {
        return Err(Violation::TooMany);
    };
    for item in items {
        let Some(item) = item else { continue };
        let valid = match item.operation {
            Operation::Add => !item.before_nonempty && item.after_nonempty,
            Operation::Remove => item.before_nonempty && !item.after_nonempty,
            Operation::Replace => item.before_nonempty && item.after_nonempty,
        };
        if !valid {
            return Err(Violation::OperationText);
        }
    }
    Ok(())
}
