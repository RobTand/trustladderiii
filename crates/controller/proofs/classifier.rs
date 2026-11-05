#[path = "../src/classifier/decision.rs"]
mod decision;
use decision::*;
use vstd::prelude::*;
verus! {
/// Q15/Q49/Corrections after close/#743: every original acceptance/error cover.
fn classifier_witness(case: u8) -> (result: bool)
    requires case < 14,
    ensures result,
{
    let mut c = Classification { kind: Kind::Remark, answer: Answer::Other,
        reconcile_issue: false, assessments_exact: true, selected_matches: true,
        has_applicable: false, supersedes_present: true, supersedes_applicable: true,
        closed: false, restartable: true, question_current: true, answer_offered: true,
        credit_integer: true, text_nonblank: true };
    match case {
        1 => { c.kind = Kind::Reconcile; c.reconcile_issue = true; },
        2 => { c.kind = Kind::Reconcile; },
        3 => { c.kind = Kind::Reconcile; c.reconcile_issue = true; c.assessments_exact = false; },
        4 => { c.selected_matches = false; },
        5 => { c.kind = Kind::ChangeRequest; c.has_applicable = true; c.supersedes_present = false; },
        6 => { c.supersedes_applicable = false; },
        7 => { c.kind = Kind::Restart; c.restartable = false; },
        8 => { c.kind = Kind::Stop; c.closed = true; },
        9 => { c.kind = Kind::Answer; c.question_current = false; },
        10 => { c.kind = Kind::Answer; c.answer_offered = false; },
        11 => { c.kind = Kind::Answer; c.answer = Answer::AddCredit; c.credit_integer = false; },
        12 => { c.kind = Kind::Complaint; },
        13 => { c.kind = Kind::ChangeRequest; c.text_nonblank = false; },
        _ => {},
    }
    let actual = decide(c);
    match case {
        0 => actual.is_ok() && !(c.reconcile_issue || c.kind == Kind::Reconcile),
        1 => actual.is_ok() && (c.reconcile_issue || c.kind == Kind::Reconcile),
        2 => actual == Err(Violation::ReconcileRequired), 3 => actual == Err(Violation::Assessments),
        4 => actual == Err(Violation::SelectedAnswer), 5 => actual == Err(Violation::MissingSupersedes),
        6 => actual == Err(Violation::InapplicableDecision), 7 => actual == Err(Violation::RestartUnavailable),
        8 => actual == Err(Violation::Closed), 9 => actual == Err(Violation::Question),
        10 => actual == Err(Violation::UnofferedAnswer), 11 => actual == Err(Violation::Credit),
        12 => actual == Err(Violation::Complaint), _ => actual == Err(Violation::EmptyChange),
    }
}
}
fn main() {}
