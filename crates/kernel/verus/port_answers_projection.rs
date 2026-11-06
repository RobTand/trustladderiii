//! #806: keep the large Node frame opaque across answer composition.
use super::port_answers_model::*;
use super::*;

verus! {
/// Stopping question row / #806: the raw command and current-question fences.
pub(super) open spec fn cleanup_matches(n: Node, cmd: u64, id: u64) -> bool {
    cmd > n.last_cmd && n.phase is Stopping
        && match n.question { Some(q) => q.id == id, None => false }
}

/// Retry row / #806: expose its two possible updates without other answer arms.
#[verifier::spinoff_prover]
pub(super) proof fn retry_shape(n: Node, cmd: u64, id: u64, next: u64)
    ensures commanded(n, cmd, id, Answer::Retry, next) == {
        let updated = if cmd > n.last_cmd { Node { last_cmd: cmd, ..n } } else { n };
        if cleanup_matches(n, cmd, id) { Node { question: None, ..updated } } else { updated }
    },
{
    reveal(commanded); reveal(answered);
}

/// #806: all answer handlers preserve these identity and monotone fields.
pub(super) open spec fn common(n: Node, after: Node) -> bool {
    after.proposed == n.proposed && after.adopted == n.adopted
        && after.next_seq == n.next_seq && after.credit.earned == n.credit.earned
        && after.credit.merge_credited == n.credit.merge_credited
        && after.review_stop == n.review_stop && after.repo == n.repo
        && after.footprint == n.footprint && after.credit.withheld == n.credit.withheld
        && (!after.holds || n.holds) && (!after.pool || n.pool)
        && (after.ticket.is_none() || after.ticket == n.ticket)
}

/// #806: stopping preserves answer-independent fields and clears an open question.
#[verifier::spinoff_prover]
pub(super) proof fn stopped_common(n: Node, outcome: Outcome)
    ensures common(n, retirement::stopped(n, outcome)),
        retirement::stopped(n, outcome).phase is Closed || retirement::stopped(n, outcome).phase is Stopping,
        retirement::stopped(n, outcome).attempt == n.attempt,
        retirement::stopped(n, outcome).replanned == n.replanned,
        n.question.is_none() ==> retirement::stopped(n, outcome).question.is_none(),
        n.phase == Phase::AwaitingRob ==> retirement::stopped(n, outcome).question.is_none()
            && retirement::stopped(n, outcome).phase is Stopping,
{
    reveal(retirement::stopped);
}
/// MarkHandled row / #806: raw field projection without the complete checker.
#[verifier::spinoff_prover]
pub(super) proof fn handled_common(n: Node)
    requires n.phase is Stopping,
    ensures common(n, handled(Node { question: None, ..n })),
        handled(Node { question: None, ..n }).phase == n.phase,
        handled(Node { question: None, ..n }).attempt == n.attempt,
        handled(Node { question: None, ..n }).replanned == n.replanned,
        handled(Node { question: None, ..n }).question.is_none(),
{ reveal(handled); }


/// Continuation rows / #806: preserve monotone fields independently of credit arithmetic.
#[verifier::spinoff_prover]
pub(super) proof fn continued_common(n: Node, q: Question, deposit: u8)
    requires n.phase == Phase::AwaitingRob,
    ensures common(n, continued(n, q, deposit)),
        continued(n, q, deposit).attempt == n.attempt,
        continued(n, q, deposit).replanned == n.replanned,
        continued(n, q, deposit).question.is_none(),
{ reveal(continued); reveal(rephased); reveal(retirement::stopped); reveal(funded); }


/// #806 continue row: isolate command and question matching from field laws.
#[verifier::spinoff_prover]
pub(super) proof fn continue_shape(n: Node, cmd: u64, id: u64, next: u64)
    ensures commanded(n, cmd, id, Answer::Continue, next) == {
        let updated = if cmd > n.last_cmd { Node { last_cmd: cmd, ..n } } else { n };
        if cmd > n.last_cmd && n.phase == Phase::AwaitingRob {
            match n.question { Some(q) => if q.id == id && offered(q.kind, Answer::Continue) {
                continued(updated, q, 0)
            } else { updated }, None => updated }
        } else { updated }
    },
{ reveal(commanded); reveal(answered); }

/// #806 try_again row: isolate command and question matching from field laws.
#[verifier::spinoff_prover]
pub(super) proof fn try_again_shape(n: Node, cmd: u64, id: u64, next: u64)
    ensures commanded(n, cmd, id, Answer::TryAgain, next) == {
        let updated = if cmd > n.last_cmd { Node { last_cmd: cmd, ..n } } else { n };
        if cmd > n.last_cmd && n.phase == Phase::AwaitingRob {
            match n.question { Some(q) => if q.id == id && offered(q.kind, Answer::TryAgain) {
                continued(Node { attempt: 1, review_restarted: false, round: 0, replanned: false, ..updated }, Question { cost: if q.cost > 1 { q.cost } else { 1 }, ..q }, 0)
            } else { updated }, None => updated }
        } else { updated }
    },
{ reveal(commanded); reveal(answered); }

/// #806 add_credit row: isolate command and question matching from field laws.
#[verifier::spinoff_prover]
pub(super) proof fn add_credit_shape(n: Node, cmd: u64, id: u64, next: u64, k: u8)
    ensures commanded(n, cmd, id, Answer::AddCredit(k), next) == {
        let updated = if cmd > n.last_cmd { Node { last_cmd: cmd, ..n } } else { n };
        if cmd > n.last_cmd && n.phase == Phase::AwaitingRob {
            match n.question { Some(q) => if q.id == id && offered(q.kind, Answer::AddCredit(k)) {
                continued(updated, q, k)
            } else { updated }, None => updated }
        } else { updated }
    },
{ reveal(commanded); reveal(answered); }

/// Unclear row / #806: expose only its command and clarification cases.
#[verifier::spinoff_prover]
pub(super) proof fn unclear_shape(n: Node, cmd: u64, id: u64, next: u64)
    ensures commanded(n, cmd, id, Answer::Unclear, next) == {
        let updated = if cmd > n.last_cmd { Node { last_cmd: cmd, ..n } } else { n };
        if cmd > n.last_cmd && clarifies(n, id, Answer::Unclear) {
            Node { question: Some(Question { id: next, clarified: true, ..n.question.unwrap() }), ..updated }
        } else { updated }
    },
{ reveal(commanded); reveal(answered); }

}
