#[path = "../src/reviewer/decision.rs"]
mod decision;
use decision::*;
use vstd::prelude::*;
verus! {
/// Reviewing/Q44/#743: all eight original covers, including the real repair text.
fn reviewer_witness(case: u8) -> (result: bool)
    requires case < 8,
    ensures result,
{
    if case < 4 {
        let verdict = match case { 1 => Verdict::Changes, 2 => Verdict::OutOfRoom, _ => Verdict::Approve };
        let actual = head(verdict, case != 3);
        let message = Violation::Head.message();
        assert(message@.len() > 0);
        if case == 3 { actual == Err(Violation::Head) }
        else { actual.is_ok() && verdict == match case { 1 => Verdict::Changes, 2 => Verdict::OutOfRoom, _ => Verdict::Approve } }
    } else {
        let f = Finding { known: case != 6, scope: case != 5, quoted: case != 7 };
        let actual = finding(f);
        match case {
            4 => actual.is_ok() && f.scope,
            5 => actual.is_ok() && !f.scope,
            6 => actual == Err(Violation::Citation),
            _ => actual == Err(Violation::Scope),
        }
    }
}
/// Reviewing/Q44/#743: preserve the original totality and soundness assertions.
fn reviewer_total(verdict: Verdict, matches: bool, f: Finding) {
    let actual_head = head(verdict, matches);
    assert(actual_head.is_err() == (verdict == Verdict::Approve && !matches));
    let actual = finding(f);
    assert(actual.is_ok() == (f.known && (!f.scope || f.quoted)));
    if !f.known { assert(actual == Err(Violation::Citation)); }
    else if f.scope && !f.quoted { assert(actual == Err(Violation::Scope)); }
    let message = Violation::Head.message();
    assert(message@.len() > 0);
}
}
fn main() {}
