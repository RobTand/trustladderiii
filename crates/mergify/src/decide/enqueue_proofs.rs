//! Scalar inputs range over their full domains. Each transition emits at most
//! three effects. Unwind 4 covers those three slots and the loop exit; the
//! remaining proofs each exercise one transition, with no temporal loop.
//!
//! Property traceability:
//! - `enqueue_required_check`: Q47, distinguish code failures from runner failures.
use super::enqueue::*;

macro_rules! proof {
    ($name:ident, $body:block) => {
        #[kani::proof]
        #[kani::unwind(4)]
        fn $name() $body
    };
}

proof!(enqueue_required_check, {
    use super::{App, Conclusion, Status};
    let check: super::CheckView = kani::any();
    let runner: bool = kani::any();
    let expected = match (check.status, check.conclusion) {
        (
            Status::Completed,
            Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped),
        ) => RequiredState::Passed,
        (Status::Completed, Some(Conclusion::ActionRequired | Conclusion::Stale)) => {
            RequiredState::Pending
        }
        (Status::Completed, Some(conclusion)) => {
            if check.app == App::Actions
                && (matches!(
                    conclusion,
                    Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure
                ) || conclusion == Conclusion::Failure && runner)
            {
                RequiredState::Infrastructure
            } else {
                RequiredState::Tests
            }
        }
        (Status::Completed, None) => RequiredState::Unknown,
        _ => RequiredState::Pending,
    };
    assert_eq!(
        required_check(check, runner),
        expected,
        "required check misclassified"
    );
    kani::cover!(expected == RequiredState::Passed, "head check passed");
    kani::cover!(expected == RequiredState::Pending, "head check pending");
    kani::cover!(expected == RequiredState::Unknown, "no conclusion");
    kani::cover!(expected == RequiredState::Tests, "test failure");
    kani::cover!(
        expected == RequiredState::Infrastructure && check.conclusion == Some(Conclusion::Failure),
        "runner annotation"
    );
    kani::cover!(
        expected == RequiredState::Infrastructure
            && check.conclusion == Some(Conclusion::Cancelled),
        "cancelled runner"
    );
});
