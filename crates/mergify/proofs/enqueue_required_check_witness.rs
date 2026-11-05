use crate::{App, CheckView, Conclusion, Head, Status};
use crate::check::Name;
use crate::enqueue::{required_check, RequiredState};
use vstd::prelude::*;

verus! {
/// Direct replacement of the old Kani assertion over arbitrary CheckView and
/// runner inputs. Its independent oracle retains every arm of that assertion.
fn oracle(check: CheckView, runner: bool) {
    let actual = required_check(check, runner);
    assert(actual == crate::required_check_spec::expected(check, runner));
}

/// Port of enqueue_required_check's six covers at base 504f10b0:343-354.
/// Q47, docs/state-machine.md:681-688; state-table.md:540-541,575.
/// Exact enum partition is preserved existing behaviour (no rule).
/// Full CheckView/u64 domain is proved by the real function's ensures;
/// these concrete executions establish every old cover, without an unwind.
fn covers() {
    let mut check = CheckView { id: 0, head: Head::Current, app: App::Actions,
        app_key: 0, name: Name::Other, name_key: 0, required: true,
        status: Status::Completed, conclusion: Some(Conclusion::Success), completed_at: None };
    let r1 = required_check(check, false);
    assert(r1 == RequiredState::Passed);
    check.status = Status::InProgress;
    let r2 = required_check(check, false);
    assert(r2 == RequiredState::Pending);
    check.status = Status::Completed;
    check.conclusion = None;
    let r3 = required_check(check, false);
    assert(r3 == RequiredState::Unknown);
    check.conclusion = Some(Conclusion::Failure);
    let r4 = required_check(check, false);
    assert(r4 == RequiredState::Tests);
    let r5 = required_check(check, true);
    assert(r5 == RequiredState::Infrastructure);
    assert(check.conclusion == Some(Conclusion::Failure));
    check.conclusion = Some(Conclusion::Cancelled);
    let r6 = required_check(check, false);
    assert(r6 == RequiredState::Infrastructure);
    assert(check.conclusion == Some(Conclusion::Cancelled));
}
}
