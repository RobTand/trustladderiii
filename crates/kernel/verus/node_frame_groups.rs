use super::*;
use vstd::prelude::*;

verus! {
#[verifier::opaque]
pub open spec fn ops_same(b: Node, a: Node) -> bool {
    a.phase == b.phase
        && a.repo == b.repo
        && a.footprint == b.footprint
        && a.effect == b.effect
        && a.ticket == b.ticket
        && a.holds == b.holds
        && a.paused == b.paused
        && a.question == b.question
        && a.waiting_incident == b.waiting_incident
        && a.pos == b.pos
}

#[verifier::opaque]
pub open spec fn ledger_same(b: Node, a: Node) -> bool {
    a.credit == b.credit
}

#[verifier::opaque]
pub open spec fn counters_same(b: Node, a: Node) -> bool {
    a.generation == b.generation
        && a.next_seq == b.next_seq
        && a.last_cmd == b.last_cmd
        && a.attempt == b.attempt
        && a.replanned == b.replanned
}

#[verifier::opaque]
pub open spec fn pr_same(b: Node, a: Node) -> bool {
    a.proposed == b.proposed
        && a.adopted == b.adopted
        && a.review_stop == b.review_stop
}

}
