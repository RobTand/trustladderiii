//! K-required-check: port of base 504f10b0 enqueue_proofs.rs:311-356.
//! Q47 (docs/state-machine.md:681-688), state-table.md:540-541,549,575
//! distinguish code and infrastructure failures. Q52 (state-machine.md:712-722,
//! state-table.md:542-544) concerns absent required checks, handled elsewhere.
//! #447 and state-table.md:545,578 concern blocked queue incidents; #608 records
//! served incident RobTand/prismabuild#2705, whose public-view PR is not a
//! classifier rule. The exact enum partition, including unknown conclusions,
//! is preserved existing behaviour (no rule). The full-domain contract is
//! inline on the real required_check body; covers are in the witness module.
use crate::{App, CheckView, Conclusion, Status};
use crate::enqueue::RequiredState;
use vstd::prelude::*;

verus! {
/// K-required-check: Q47, docs/state-machine.md:681-688 and state-table.md:540-541,549,575.
/// The exact enum partition is preserved existing behaviour (no rule).
/// #447/state-table.md:545,578 concern queue conditions; #608 records served
/// RobTand/prismabuild#2705, not a classifier rule. All inputs are unbounded.
/// This is the retired Kani oracle, independent of the executable classifier.
pub open spec fn expected(check: CheckView, runner: bool) -> RequiredState {
    match (check.status, check.conclusion) {
        (Status::Completed, Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)) => RequiredState::Passed,
        (Status::Completed, Some(Conclusion::ActionRequired | Conclusion::Stale)) => RequiredState::Pending,
        (Status::Completed, Some(conclusion)) => {
            if check.app == App::Actions &&
                (matches!(conclusion, Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure)
                    || conclusion == Conclusion::Failure && runner) {
                RequiredState::Infrastructure
            } else { RequiredState::Tests }
        },
        (Status::Completed, None) => RequiredState::Unknown,
        _ => RequiredState::Pending,
    }
}
}
