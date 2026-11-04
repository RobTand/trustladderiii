//! The parser reduces unbounded strings and sets to the finite facts in
//! Classification. This proof exhausts those facts, including inconsistent
//! combinations, without a request-count or string-length bound.
use super::decision::*;

/// Encodes Q15 comment classification, Q49 restart availability, and the
/// state-table's "Corrections after close" rules, in the existing repair order.
#[kani::proof]
// Twelve possible violations plus the loop's terminating check.
#[kani::unwind(13)]
fn classification_total_and_sound() {
    let c: Classification = kani::any();
    let reconcile = c.reconcile_issue || c.kind == Kind::Reconcile;
    let change = matches!(c.kind, Kind::ChangeRequest)
        || (c.kind == Kind::Answer && c.answer == Answer::ChangeRequest);
    let normal = !reconcile;
    let rules = [
        (
            reconcile && !(c.reconcile_issue && c.kind == Kind::Reconcile),
            Violation::ReconcileRequired,
        ),
        (reconcile && !c.assessments_exact, Violation::Assessments),
        (normal && !c.selected_matches, Violation::SelectedAnswer),
        (
            normal && change && c.has_applicable && !c.supersedes_present,
            Violation::MissingSupersedes,
        ),
        (
            normal && !c.supersedes_applicable,
            Violation::InapplicableDecision,
        ),
        (
            normal && c.kind == Kind::Restart && !c.restartable,
            Violation::RestartUnavailable,
        ),
        (
            normal
                && c.closed
                && !matches!(
                    c.kind,
                    Kind::Restart | Kind::Complaint | Kind::Remark | Kind::Unsure
                ),
            Violation::Closed,
        ),
        (
            normal && c.kind == Kind::Answer && !c.question_current,
            Violation::Question,
        ),
        (
            normal && c.kind == Kind::Answer && c.answer != Answer::Unclear && !c.answer_offered,
            Violation::UnofferedAnswer,
        ),
        (
            normal && c.kind == Kind::Answer && c.answer == Answer::AddCredit && !c.credit_integer,
            Violation::Credit,
        ),
        (
            normal && c.kind == Kind::Complaint && !c.closed,
            Violation::Complaint,
        ),
        (normal && change && !c.text_nonblank, Violation::EmptyChange),
    ];
    let mut expected = Ok(());
    for (rejects, violation) in rules {
        if rejects && expected.is_ok() {
            expected = Err(violation);
        }
    }
    let actual = decide(c);
    assert_eq!(
        actual, expected,
        "decision must satisfy the ordered repair rules"
    );
    kani::cover!(actual.is_ok() && !reconcile, "ordinary reply accepted");
    kani::cover!(actual.is_ok() && reconcile, "reconciliation accepted");
    kani::cover!(
        actual == Err(Violation::ReconcileRequired),
        "reconcile required"
    );
    kani::cover!(
        actual == Err(Violation::Assessments),
        "exact reconciliation coverage"
    );
    kani::cover!(
        actual == Err(Violation::SelectedAnswer),
        "selected answer immutable"
    );
    kani::cover!(
        actual == Err(Violation::MissingSupersedes),
        "replacement assesses decisions"
    );
    kani::cover!(
        actual == Err(Violation::InapplicableDecision),
        "only applicable decisions"
    );
    kani::cover!(
        actual == Err(Violation::RestartUnavailable),
        "restart offered"
    );
    kani::cover!(actual == Err(Violation::Closed), "closed controls rejected");
    kani::cover!(actual == Err(Violation::Question), "only current question");
    kani::cover!(
        actual == Err(Violation::UnofferedAnswer),
        "only offered answer"
    );
    kani::cover!(actual == Err(Violation::Credit), "integer credit");
    kani::cover!(
        actual == Err(Violation::Complaint),
        "complaints on closed requests"
    );
    kani::cover!(
        actual == Err(Violation::EmptyChange),
        "nonblank replacement"
    );
}
