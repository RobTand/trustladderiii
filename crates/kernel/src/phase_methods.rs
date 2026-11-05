//! #771: unchanged phase and effect methods, independently gated.
use super::*;

impl EarlyEnd {
    kernel_fn! {
    /// Ingest early-end rows / #771: preserve the recorded terminal outcome.
    pub fn outcome(self) -> (result: Outcome) [
        ensures result == match self {
            EarlyEnd::Duplicate => Outcome::Duplicate,
            EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
            EarlyEnd::NotActionable => Outcome::NotActionable,
        },
    ] {
        match self {
            EarlyEnd::Duplicate => Outcome::Duplicate,
            EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
            EarlyEnd::NotActionable => Outcome::NotActionable,
        }
    }
    }
}

impl Resume {
    kernel_fn! {
        pub fn phase(self) -> (result: Phase) [
            ensures result == resumed_phase(self),
        ] {
            match self {
                Resume::Making => Phase::Making,
                Resume::Checking => Phase::Checking,
                Resume::Reviewing => Phase::Reviewing,
                Resume::Publishing => Phase::Publishing,
                Resume::Rebasing => Phase::Rebasing,
            }
        }
    }
}

impl Phase {
    kernel_fn! {
        /// The phases in which a leaf holds a slot and its files.
        pub fn holding(self) -> (result: bool) [
            ensures result == holding(self),
        ] {
            matches!(
                self,
                Phase::Making
                    | Phase::Checking
                    | Phase::Reviewing
                    | Phase::Publishing
                    | Phase::Queued
                    | Phase::Rebasing
            )
        }
    }

    kernel_fn! {
        /// Where a holding leaf picks up again after waiting in Ready.
        pub fn resume(self) -> (result: Option<Resume>) [
            ensures result == match self {
                Phase::Making => Some(Resume::Making),
                Phase::Checking => Some(Resume::Checking),
                Phase::Reviewing => Some(Resume::Reviewing),
                Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
                Phase::Rebasing => Some(Resume::Rebasing),
                _ => None,
            },
        ] {
            match self {
                Phase::Making => Some(Resume::Making),
                Phase::Checking => Some(Resume::Checking),
                Phase::Reviewing => Some(Resume::Reviewing),
                Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
                Phase::Rebasing => Some(Resume::Rebasing),
                _ => None,
            }
        }
    }

    kernel_fn! {
        pub fn is_closed(self) -> (result: bool) [
            ensures result == (self is Closed),
        ] {
            matches!(self, Phase::Closed(_))
        }
    }

    kernel_fn! {
        pub(super) fn is_stopping(self) -> (result: bool) [
            ensures result == (self is Stopping),
        ] {
            matches!(self, Phase::Stopping(_))
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Phase::Ingesting => "Ingesting",
            Phase::Auditing => "Auditing",
            Phase::Unplanned => "Unplanned",
            Phase::Planning { .. } => "Planning",
            Phase::Ready(_) => "Ready",
            Phase::Making => "Making",
            Phase::Checking => "Checking",
            Phase::Reviewing => "Reviewing",
            Phase::Publishing => "Publishing",
            Phase::Queued => "Queued",
            Phase::Rebasing => "Rebasing",
            Phase::AwaitingRob => "AwaitingRob",
            Phase::Stopping(_) => "Stopping",
            Phase::Closed(_) => "Closed",
        }
    }
}

impl EffectKind {
    kernel_fn! {
        /// Effects that start an operation. The operation's reply moves the node on.
        pub(super) fn starts_operation(self) -> (result: bool) [
            ensures result == matches!(self, EffectKind::StartRun(_)
                | EffectKind::SubmitChecks | EffectKind::StartRebase),
        ] {
            matches!(
                self,
                EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase
            )
        }
    }
}
