//! #806 W7: exact adoption preparation frames over the real acknowledgement path.
use super::*;

verus! {
/// State-table Making/PrepareWorktree / #806: the unchanged PrepareWorktree agenda branch.
#[verifier::opaque]
pub(super) open spec fn agenda(n: Node) -> Node {
    if !n.adopted && n.proposed.is_some() {
        retirement::phased(Node { adopted: true, head: n.proposed, checked: None, ..n }, Phase::Checking)
    } else { Node { pos: 1, ..n } }
}

/// State-table Making/PrepareWorktree / #806: confirmed preparation resets delivery failures first.
#[verifier::opaque]
pub(super) open spec fn confirmed(n: Node) -> Node {
    agenda(Node { fx_failures: 0, fx_incidents: 0, ..n })
}

/// State-table Making/PrepareWorktree / #806: the real matching acknowledgement clears its effect first.
#[verifier::opaque]
pub(super) open spec fn acknowledged(n: Node) -> Node {
    confirmed(Node { effect: None, ..n })
}

/// #806 original adoption assumptions, without small-head or counter bounds.
pub(super) open spec fn preparing(n: Node) -> bool {
    n.phase == Phase::Making && n.pos == 0 && n.proposed.is_some()
        && n.effect.is_some() && n.effect.unwrap().kind == EffectKind::PrepareWorktree
        && !n.effect.unwrap().cancelling
}

/// #806 adoption: W3's neutral node fields, with no dependency on its internal agenda model.
#[verifier::opaque]
pub(super) open spec fn fields(b: Node, a: Node) -> bool {
    a.adopted
        && a.head == (if b.adopted { b.head } else { b.proposed })
        && a.phase == (if b.adopted { Phase::Making } else { Phase::Checking })
        && a.pos == (if b.adopted { 1u8 } else { 0u8 })
        && a.generation as int == b.generation as int + (if b.adopted { 0int } else { 1int })
        && a.ticket == (if b.adopted { b.ticket } else { None })
        && a.checked == (if b.adopted { b.checked } else { None })
        && a.effect.is_none() && a.fx_failures == 0 && a.fx_incidents == 0
        && a.repo == b.repo
        && a.footprint == b.footprint
        && a.next_seq == b.next_seq
        && a.run == b.run
        && a.job == b.job
        && a.pool == b.pool
        && a.holds == b.holds
        && a.paused == b.paused
        && a.last_cmd == b.last_cmd
        && a.question == b.question
        && a.attempt == b.attempt
        && a.round == b.round
        && a.replanned == b.replanned
        && a.plan_retries == b.plan_retries
        && a.audit_rounds == b.audit_rounds
        && a.early_end == b.early_end
        && a.run_failures == b.run_failures
        && a.run_incidents == b.run_incidents
        && a.waiting_incident == b.waiting_incident
        && a.incident_needs_rob == b.incident_needs_rob
        && a.proposed == b.proposed
        && a.approved == b.approved
        && a.merged == b.merged
        && a.pr_open == b.pr_open
        && a.had_pr == b.had_pr
        && a.worktree == b.worktree
        && a.rechecks == b.rechecks
        && a.review_restarted == b.review_restarted
        && a.credit == b.credit
        && a.priority == b.priority
        && a.review_stop == b.review_stop
        && a.planning_started == b.planning_started
        && a.waiting_prerequisite == b.waiting_prerequisite
}

/// #806 adoption: reconstruct the exact local result only from the neutral field clauses.
pub(super) proof fn from_fields(b: Node, a: Node)
    requires preparing(b), b.generation < u64::MAX, fields(b, a),
    ensures a == acknowledged(b),
{
    reveal(fields); reveal(acknowledged); reveal(confirmed); reveal(agenda);
    reveal(retirement::phased); reveal(preparing);
}

/// #806 real apply bridge: extract the public Ack fields and complete output stutter.
/// The node projection is shared with the direct on_ack consumer.
pub(super) proof fn local_fields(b: Node, a: Node, qb: u64, qa: u64,
    i: usize, seq: u64, ob: Outbox, oa: Outbox)
    requires preparing(b), b.generation < u64::MAX, seq == b.effect.unwrap().seq,
        port_acks_specs::ack_local(b, a, qb, qa, i, seq, Ack::Confirmed, ob, oa),
    ensures fields(b, a), qa == qb, oa.len == ob.len, oa.items@ == ob.items@,
{
    reveal(port_acks_specs::ack_local); reveal(port_acks_specs::ack_output);
    reveal(port_acks_specs::ack_emissions); reveal(port_acks_specs::appended);
    reveal(preparing);
    assert(oa.items@ =~= ob.items@);
    assert(port_acks_specs::ack_local(b, a, qb, qb, i, seq, Ack::Confirmed, ob, ob));
    port_acks_frames::ack_adoption_fields(b, a, qb, i, seq, ob);
    reveal(fields);
}
}
