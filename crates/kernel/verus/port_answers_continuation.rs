//! #806: opaque node-only composition of the real paid continuation.
use super::port_answers_model::*;
use super::*;

verus! {
/// Q18 / #806: payment and question closure occur before continuation dispatch.
#[verifier::opaque]
pub(super) open spec fn pay_post(n: Node, after: Node, q: Question, deposit: u8) -> bool {
    after == Node { credit: funded(n.credit, deposit, q.cost), question: None, ..n }
}

/// #806: the verbatim continuation dispatch, including already stopping/closed nodes.
#[verifier::opaque]
pub(super) open spec fn resumed(n: Node, q: Question) -> Node {
    match q.cont {
        Cont::Ingesting => rephased(n, Phase::Ingesting),
        Cont::Auditing => rephased(n, Phase::Auditing),
        Cont::Unplanned => rephased(n, Phase::Unplanned),
        Cont::Planning { must_split } => rephased(Node { plan_retries: 0, ..n }, Phase::Planning { must_split }),
        Cont::Ready(r) => rephased(n, Phase::Ready(r)),
        Cont::Stop(o) => retirement::stopped(n, o),
        Cont::Nowhere => retirement::stopped(n, Outcome::Stopped),
    }
}

/// #806: hide exact Node reconstruction from the World-level continuation.
#[verifier::opaque]
pub(super) open spec fn resume_post(n: Node, after: Node, q: Question) -> bool {
    after == resumed(n, q)
}

/// #806: the real go_on result, opened only by the answer bridge.
#[verifier::opaque]
pub(super) open spec fn continuation_post(n: Node, after: Node, q: Question, deposit: u8) -> bool {
    after == continued(n, q, deposit)
}

/// Q18 / #771: the primitive ledger contract determines the exact funded ledger.
#[verifier::spinoff_prover]
pub(super) proof fn seal_pay(n: Node, after: Node, q: Question, deposit: u8)
    requires after == (Node { credit: after.credit, question: None, ..n }),
        after.credit.ready == n.credit.ready, after.credit.start == n.credit.start,
        after.credit.earned == n.credit.earned, after.credit.withheld == n.credit.withheld,
        after.credit.merge_credited == n.credit.merge_credited,
        after.credit.spent as int == n.credit.spent as int + q.cost as int,
        after.credit.added as int == n.credit.added as int + deposit as int
            + if q.cost as int > n.credit.start as int + n.credit.earned as int
                + n.credit.added as int + deposit as int - n.credit.spent as int {
                q.cost as int - (n.credit.start as int + n.credit.earned as int
                    + n.credit.added as int + deposit as int - n.credit.spent as int)
            } else { 0int },
    ensures pay_post(n, after, q, deposit),
{ reveal(pay_post); reveal(funded); }

/// #806: equate the two helper results without opening their Node models in go_on.
#[verifier::spinoff_prover]
pub(super) proof fn compose(n: Node, paid: Node, after: Node, q: Question, deposit: u8)
    requires pay_post(n, paid, q, deposit), resume_post(paid, after, q),
    ensures continuation_post(n, after, q, deposit),
{ reveal(pay_post); reveal(resume_post); reveal(resumed); reveal(continuation_post); reveal(continued); }

/// #771 / #806: retain all continuation field and arithmetic bounds separately from ranks.
#[verifier::spinoff_prover]
pub(super) proof fn legacy_fields(n: Node, after: Node, q: Question, deposit: u8)
    requires continuation_post(n, after, q, deposit),
        n.generation < u64::MAX, q.cost <= 1,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int >= n.credit.spent as int,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures
        after.repo == n.repo, after.footprint == n.footprint,
        after.generation as int <= n.generation as int + 1,
        after.next_seq == n.next_seq,
        after.credit.start == n.credit.start, after.credit.earned == n.credit.earned,
        after.credit.added as int <= n.credit.added as int + 255,
        after.credit.spent as int <= n.credit.spent as int + 1,
        after.last_cmd == n.last_cmd, after.question.is_none(),
{
    reveal(continuation_post); reveal(continued); reveal(rephased);
    reveal(retirement::stopped); reveal(funded);
}

/// #805 / #806: preserve settling shape independently of ledger bounds.
#[verifier::spinoff_prover]
pub(super) proof fn legacy_shape(n: Node, after: Node, q: Question, deposit: u8)
    requires continuation_post(n, after, q, deposit),
    ensures settle_shape(n) ==> settle_shape(after),
{
    reveal(continuation_post); reveal(continued); reveal(rephased);
    reveal(retirement::stopped); reveal(funded);
}

/// #805 / #806: a matching unpaused answer strictly decreases the legacy distance.
#[verifier::spinoff_prover]
pub(super) proof fn legacy_progress(n: Node, after: Node, q: Question, deposit: u8)
    requires continuation_post(n, after, q, deposit),
        n.phase == Phase::AwaitingRob, n.question == Some(q), !n.paused,
        !n.waiting_incident, n.effect.is_none(),
    ensures distance(after) < distance(n),
{
    reveal(continuation_post); reveal(continued); reveal(rephased);
    reveal(retirement::stopped); reveal(funded);
    reveal(distance);
}

/// #806: seal dispatch from the primitive phase and stopping field contracts.
#[verifier::spinoff_prover]
pub(super) proof fn seal_resume(n: Node, after: Node, q: Question)
    requires after == match q.cont {
        Cont::Ingesting => retirement::phased(n, Phase::Ingesting),
        Cont::Auditing => retirement::phased(n, Phase::Auditing),
        Cont::Unplanned => retirement::phased(n, Phase::Unplanned),
        Cont::Planning { must_split } => retirement::phased(Node { plan_retries: 0, ..n }, Phase::Planning { must_split }),
        Cont::Ready(r) => retirement::phased(n, Phase::Ready(r)),
        Cont::Stop(o) => retirement::stopped(n, o),
        Cont::Nowhere => retirement::stopped(n, Outcome::Stopped),
    },
    ensures resume_post(n, after, q),
{ reveal(resume_post); reveal(resumed); reveal(rephased); }
}
