//! Finite reviewer decisions after wire parsing and evidence reduction.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Verdict {
    Approve,
    Changes,
    OutOfRoom,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Finding {
    pub known: bool,
    pub scope: bool,
    pub quoted: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    Head,
    Citation,
    Scope,
}

impl Violation {
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result@.len() > 0,
))]
    pub fn message(self) -> &'static str {
        #[cfg(verus_keep_ghost)]
        vstd::prelude::verus_exec_expr! { proof {
            reveal_strlit("approval names a head other than the reviewed head");
            reveal_strlit("blocking finding cites an unknown goal or standing rule");
            reveal_strlit("scope blocker must quote a changed hunk");
        } }
        match self {
            Self::Head => "approval names a head other than the reviewed head",
            Self::Citation => "blocking finding cites an unknown goal or standing rule",
            Self::Scope => "scope blocker must quote a changed hunk",
        }
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if verdict == Verdict::Approve && !matches { Err(Violation::Head) } else { Ok(()) },
))]
pub fn head(verdict: Verdict, matches: bool) -> Result<(), Violation> {
    if verdict == Verdict::Approve && !matches {
        Err(Violation::Head)
    } else {
        Ok(())
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if !f.known { Err(Violation::Citation) } else if f.scope && !f.quoted { Err(Violation::Scope) } else { Ok(()) },
))]
pub fn finding(f: Finding) -> Result<(), Violation> {
    if !f.known {
        Err(Violation::Citation)
    } else if f.scope && !f.quoted {
        Err(Violation::Scope)
    } else {
        Ok(())
    }
}
