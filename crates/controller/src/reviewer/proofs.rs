use super::decision::*;

/// State-table Reviewing and Q44: approvals bind the reviewed head; blockers
/// cite supplied goals/rules and scope blockers quote meaningful changed hunks.
#[kani::proof]
// Both decision functions are loop-free; the bound includes their terminal step.
#[kani::unwind(1)]
fn reviewer_total_and_sound() {
    let verdict: Verdict = kani::any();
    let matches: bool = kani::any();
    let f: Finding = kani::any();
    let actual_head = head(verdict, matches);
    assert_eq!(actual_head.is_err(), verdict == Verdict::Approve && !matches);
    let actual = finding(f);
    assert_eq!(actual.is_ok(), f.known && (!f.scope || f.quoted));
    if !f.known {
        assert_eq!(actual, Err(Violation::Citation));
    } else if f.scope && !f.quoted {
        assert_eq!(actual, Err(Violation::Scope));
    }
    kani::cover!(actual_head.is_ok() && verdict == Verdict::Approve, "exact head approval");
    kani::cover!(actual_head.is_ok() && verdict == Verdict::Changes, "changes accepted");
    kani::cover!(actual_head.is_ok() && verdict == Verdict::OutOfRoom, "controller exhaustion accepted");
    kani::cover!(actual_head == Err(Violation::Head), "wrong head rejected");
    kani::cover!(actual.is_ok() && f.scope, "quoted scope accepted");
    kani::cover!(actual.is_ok() && !f.scope, "ordinary citation accepted");
    kani::cover!(actual == Err(Violation::Citation), "unknown citation rejected");
    kani::cover!(actual == Err(Violation::Scope), "unquoted scope rejected");
    // Exercise repair message selection too; strings themselves are pinned by golden parity.
    assert!(!Violation::Head.message().is_empty());
}
