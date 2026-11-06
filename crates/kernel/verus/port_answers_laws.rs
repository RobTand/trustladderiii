//! #806: shared assertions split by answer, independent of the world checker.
use super::port_answers_model::*;
use super::*;
verus! {
/// #806 / Q54 change request row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn change_request_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::ChangeRequest, next), id),
        !commanded(n, cmd, id, Answer::ChangeRequest, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::ChangeRequest, next).pool || n.pool,
        commanded(n, cmd, id, Answer::ChangeRequest, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::ChangeRequest, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::ChangeRequest, next).repo == n.repo,
        commanded(n, cmd, id, Answer::ChangeRequest, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::ChangeRequest, next).credit.withheld == n.credit.withheld,
{
    reveal(shared); reveal(answered); reveal(commanded);
    if cmd > n.last_cmd {
        let updated = Node { last_cmd: cmd, ..n };
        if let Some(q) = n.question {
            if q.id == id {
                if n.phase == Phase::AwaitingRob && offered(q.kind, Answer::ChangeRequest) {
                    port_answers_projection::stopped_common(updated, Outcome::Replaced);
                }
            }
        }
    }
}

/// #806 / Q54 stop row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn stop_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::Stop, next), id),
        !commanded(n, cmd, id, Answer::Stop, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::Stop, next).pool || n.pool,
        commanded(n, cmd, id, Answer::Stop, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::Stop, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Stop, next).repo == n.repo,
        commanded(n, cmd, id, Answer::Stop, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Stop, next).credit.withheld == n.credit.withheld,
{
    reveal(shared); reveal(answered); reveal(commanded);
    if cmd > n.last_cmd {
        let updated = Node { last_cmd: cmd, ..n };
        if let Some(q) = n.question {
            if q.id == id {
                if n.phase == Phase::AwaitingRob && offered(q.kind, Answer::Stop) {
                    port_answers_projection::stopped_common(updated, Outcome::Stopped);
                }
            }
        }
    }
}

/// #806 / Q54 unclear row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn unclear_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::Unclear, next), id),
        !commanded(n, cmd, id, Answer::Unclear, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::Unclear, next).pool || n.pool,
        commanded(n, cmd, id, Answer::Unclear, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::Unclear, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Unclear, next).repo == n.repo,
        commanded(n, cmd, id, Answer::Unclear, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Unclear, next).credit.withheld == n.credit.withheld,
{
    reveal(shared);
    port_answers_projection::unclear_shape(n, cmd, id, next);
}

/// #806 / Q54 retry row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn retry_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::Retry, next), id),
        !commanded(n, cmd, id, Answer::Retry, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::Retry, next).pool || n.pool,
        commanded(n, cmd, id, Answer::Retry, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::Retry, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Retry, next).repo == n.repo,
        commanded(n, cmd, id, Answer::Retry, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Retry, next).credit.withheld == n.credit.withheld,
{
    reveal(shared);
    port_answers_projection::retry_shape(n, cmd, id, next);
}

/// #806 / Q54 retry row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn mark_handled_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::MarkHandled, next), id),
        !commanded(n, cmd, id, Answer::MarkHandled, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::MarkHandled, next).pool || n.pool,
        commanded(n, cmd, id, Answer::MarkHandled, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::MarkHandled, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::MarkHandled, next).repo == n.repo,
        commanded(n, cmd, id, Answer::MarkHandled, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::MarkHandled, next).credit.withheld == n.credit.withheld,
{
    reveal(shared);
    reveal(commanded); reveal(answered);
    if cmd > n.last_cmd {
        let updated = Node { last_cmd: cmd, ..n };
        if let Some(q) = n.question {
            if q.id == id && n.phase is Stopping {
                port_answers_projection::handled_common(Node { last_cmd: cmd, ..n });
            }
        }
    }
}


/// #806 / Q54 continue row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn continue_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::Continue, next), id),
        !commanded(n, cmd, id, Answer::Continue, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::Continue, next).pool || n.pool,
        commanded(n, cmd, id, Answer::Continue, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::Continue, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Continue, next).repo == n.repo,
        commanded(n, cmd, id, Answer::Continue, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Continue, next).credit.withheld == n.credit.withheld,
{
    port_answers_projection::continue_shape(n, cmd, id, next);
    reveal(shared);
    if cmd > n.last_cmd && n.phase == Phase::AwaitingRob {
        let updated = Node { last_cmd: cmd, ..n };
        if let Some(q) = n.question {
            if q.id == id && offered(q.kind, Answer::Continue) {
                port_answers_projection::continued_common(updated, q, 0);
            }
        }
    }
}



/// #806 / Q54 try_again row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn try_again_laws(n: Node, cmd: u64, id: u64, next: u64)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::TryAgain, next), id),
        !commanded(n, cmd, id, Answer::TryAgain, next).holds || n.holds,
        !commanded(n, cmd, id, Answer::TryAgain, next).pool || n.pool,
        commanded(n, cmd, id, Answer::TryAgain, next).ticket.is_none()
            || commanded(n, cmd, id, Answer::TryAgain, next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::TryAgain, next).repo == n.repo,
        commanded(n, cmd, id, Answer::TryAgain, next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::TryAgain, next).credit.withheld == n.credit.withheld,
{
    port_answers_projection::try_again_shape(n, cmd, id, next);
    reveal(shared);
    if cmd > n.last_cmd && n.phase == Phase::AwaitingRob {
        let updated = Node { last_cmd: cmd, ..n };
        if let Some(q) = n.question {
            if q.id == id && offered(q.kind, Answer::TryAgain) {
                port_answers_projection::continued_common(Node { attempt: 1, review_restarted: false, round: 0, replanned: false, ..updated }, Question { cost: if q.cost > 1 { q.cost } else { 1 }, ..q }, 0);
            }
        }
    }
}



/// #806 / Q54 add_credit row: every shared Kani assertion and resource frame.
#[verifier::spinoff_prover]
pub(super) proof fn add_credit_laws(n: Node, cmd: u64, id: u64, next: u64, k: u8)
    requires match n.question { Some(q) => q.id < next, None => true },
    ensures shared(n, commanded(n, cmd, id, Answer::AddCredit(k), next), id),
        !commanded(n, cmd, id, Answer::AddCredit(k), next).holds || n.holds,
        !commanded(n, cmd, id, Answer::AddCredit(k), next).pool || n.pool,
        commanded(n, cmd, id, Answer::AddCredit(k), next).ticket.is_none()
            || commanded(n, cmd, id, Answer::AddCredit(k), next).ticket == n.ticket,
        commanded(n, cmd, id, Answer::AddCredit(k), next).repo == n.repo,
        commanded(n, cmd, id, Answer::AddCredit(k), next).footprint == n.footprint,
        commanded(n, cmd, id, Answer::AddCredit(k), next).credit.withheld == n.credit.withheld,
{
    port_answers_projection::add_credit_shape(n, cmd, id, next, k);
    reveal(shared);
    if cmd > n.last_cmd && n.phase == Phase::AwaitingRob {
        let updated = Node { last_cmd: cmd, ..n };
        if let Some(q) = n.question {
            if q.id == id && offered(q.kind, Answer::AddCredit(k)) {
                port_answers_projection::continued_common(updated, q, k);
            }
        }
    }
}



}
