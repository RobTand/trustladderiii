//! Finite reviewer decisions after wire parsing and evidence reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Verdict {
    Approve,
    Changes,
    OutOfRoom,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Finding {
    pub known: bool,
    pub scope: bool,
    pub quoted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    Head,
    Citation,
    Scope,
}

impl Violation {
    pub fn message(self) -> &'static str {
        match self {
            Self::Head => "approval names a head other than the reviewed head",
            Self::Citation => "blocking finding cites an unknown goal or standing rule",
            Self::Scope => "scope blocker must quote a changed hunk",
        }
    }
}

pub fn head(verdict: Verdict, matches: bool) -> Result<(), Violation> {
    if verdict == Verdict::Approve && !matches {
        Err(Violation::Head)
    } else {
        Ok(())
    }
}

pub fn finding(f: Finding) -> Result<(), Violation> {
    if !f.known {
        Err(Violation::Citation)
    } else if f.scope && !f.quoted {
        Err(Violation::Scope)
    } else {
        Ok(())
    }
}
