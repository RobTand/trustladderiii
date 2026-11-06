use vstd::prelude::*;
#[path = "../src/judge.rs"]
mod judge;
use judge::{Input, Kind, Outcome, Reason, judge};

verus! {
/// Q40/#822: every valid Check input, not a sampled or bounded subset.
fn regression_properties(input: Input)
    requires input.forced.is_none() || matches!(input.kind, Kind::Demonstrating),
{
    let out = judge(input);
    assert(out.pre_existing == (matches!(input.kind, Kind::Regression)
        && matches!(input.base, Outcome::Assertion | Outcome::Error)));
    if matches!(input.base, Outcome::AddedName) {
        assert(!out.pre_existing);
    }
    if matches!(input.kind, Kind::Regression) && out.passed {
        assert(matches!(input.base,
            Outcome::Passed | Outcome::Assertion | Outcome::AddedName | Outcome::Error));
    }
}

/// State-table Checks: NoReport never supplies a passing valid check.
fn missing_report_is_not_a_pass(input: Input)
    requires input.forced.is_none() || matches!(input.kind, Kind::Demonstrating),
{
    let out = judge(input);
    if out.infrastructure { assert(!out.passed); }
}

/// Q40/Q42/Q55/#713: RED assertion requires declaration-bound matching and green heads.
fn demonstration_properties(input: Input)
    requires input.forced.is_none() || matches!(input.kind, Kind::Demonstrating),
{
    let out = judge(input);
    if out.passed && matches!(input.kind, Kind::Demonstrating | Kind::Reproducing) {
        assert(matches!(input.base, Outcome::Assertion | Outcome::AddedName));
        assert(matches!(input.head, Outcome::Passed));
        if input.forced.is_some() {
            assert(matches!(input.base, Outcome::Assertion));
            assert(matches!(input.forced, Some(Outcome::Passed)));
        }
        if matches!(input.base, Outcome::Assertion) {
            assert(matches!(input.reason, Reason::Matched | Reason::DeclaredSymptom));
        }
    }
    if matches!(input.kind, Kind::BehaviorPreserving) && out.passed {
        assert(matches!(input.base, Outcome::Passed));
        assert(matches!(input.head, Outcome::Passed));
    }
}

/// #822/#749: reachable pass, rejection, pre-existing failure and forced success witnesses.
fn judge_witnesses() {
    let added = judge(Input { kind: Kind::Regression, base: Outcome::AddedName,
        head: Outcome::Passed, forced: None, reason: Reason::Undeclared });
    assert(added.passed && !added.pre_existing && !added.infrastructure);
    let pre = judge(Input { kind: Kind::Regression, base: Outcome::Error,
        head: Outcome::Assertion, forced: None, reason: Reason::Undeclared });
    assert(pre.passed && pre.pre_existing);
    let forced = judge(Input { kind: Kind::Demonstrating, base: Outcome::Assertion,
        head: Outcome::Passed, forced: Some(Outcome::Passed), reason: Reason::Matched });
    assert(forced.passed);
    let no_reason = judge(Input { kind: Kind::Demonstrating, base: Outcome::Assertion,
        head: Outcome::Passed, forced: Some(Outcome::Passed), reason: Reason::Undeclared });
    assert(!no_reason.passed);
    let missing = judge(Input { kind: Kind::Regression, base: Outcome::NoReport,
        head: Outcome::Passed, forced: None, reason: Reason::Undeclared });
    assert(missing.infrastructure && !missing.passed);
}
}
fn main() {}
