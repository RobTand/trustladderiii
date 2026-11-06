//! #860: small local frames and complete production-checker composition.
use super::port_t860_contracts::*;
use super::*;

verus! {
/// Auditing / #832-A / #860: first and repeated disagreements preserve every node rule.
/// One real transition, no unwind; audit_rounds saturates at u8::MAX.
pub(super) proof fn admitted(w: World, n: Node, generation: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        n.phase == Phase::Auditing, active(n, generation),
    ensures retirement::node_rules(w, reply_node(n, generation)),
        node_change(n, reply_node(n, generation), w.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, 0),
        !reply_node(n, generation).holds, !reply_node(n, generation).pool,
        reply_node(n, generation).ticket.is_none(),
        reply_node(n, generation).credit == n.credit,
        reply_node(n, generation).repo == n.repo, reply_node(n, generation).footprint == n.footprint,
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(reply_node); reveal(audit_node); reveal(ended);
}

/// Q50 / #860: the first rejected reply retries the operation without work retries.
pub(super) proof fn rejected_once(w: World, n: Node, generation: u64)
    requires retirement::node_rules(w, n), node_headroom(n), active(n, generation),
        !(n.phase is Stopping), n.phase != Phase::Auditing, n.run_failures == 0,
    ensures retirement::node_rules(w, reply_node(n, generation)),
        node_change(n, reply_node(n, generation), w.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, 0),
        reply_node(n, generation).holds == n.holds, !reply_node(n, generation).pool,
        reply_node(n, generation).ticket.is_none(),
{
    reveal(retirement::node_rules); reveal(reply_node); reveal(failed_node);
}

/// Q50 / #860: a second rejected reply raises the existing bounded incident.
pub(super) proof fn rejected_incident(w: World, n: Node, generation: u64)
    requires retirement::node_rules(w, n), node_headroom(n), active(n, generation),
        !(n.phase is Stopping), n.phase != Phase::Auditing, n.run_failures == 1,
    ensures retirement::node_rules(w, reply_node(n, generation)),
        node_change(n, reply_node(n, generation), w.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, 0),
        !reply_node(n, generation).holds, !reply_node(n, generation).pool,
        reply_node(n, generation).ticket.is_none(),
{
    reveal(retirement::node_rules); reveal(reply_node); reveal(failed_node); reveal(retirement::incident);
}

/// Stopping and stale reply rows / #860: only matching stopping ownership retires.
pub(super) proof fn inactive(w: World, n: Node, generation: u64)
    requires retirement::node_rules(w, n), n.phase is Stopping || !active(n, generation),
    ensures retirement::node_rules(w, reply_node(n, generation)),
        node_change(n, reply_node(n, generation), w.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, 0),
        reply_node(n, generation).holds == n.holds,
        (reply_node(n, generation).pool ==> n.pool),
        reply_node(n, generation).ticket == n.ticket,
{
    reveal(retirement::node_rules); reveal(reply_node);
}

/// #860: select a local lemma without unrolling the bounded original world.
pub(super) proof fn actor(w: World, n: Node, generation: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
    ensures retirement::node_rules(w, reply_node(n, generation)),
        node_change(n, reply_node(n, generation), w.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, 0),
        (reply_node(n, generation).holds ==> n.holds),
        (reply_node(n, generation).pool ==> n.pool),
        (reply_node(n, generation).ticket.is_some() ==> reply_node(n, generation).ticket == n.ticket),
        reply_node(n, generation).repo == n.repo, reply_node(n, generation).footprint == n.footprint,
        reply_node(n, generation).credit == n.credit,
{
    if n.phase is Stopping || !active(n, generation) { inactive(w, n, generation); }
    else if n.phase == Phase::Auditing { admitted(w, n, generation); }
    else {
        reveal(retirement::node_rules);
        if n.run_failures == 0 { rejected_once(w, n, generation); }
        else { rejected_incident(w, n, generation); }
    }
    reveal(reply_node); reveal(failed_node); reveal(audit_node); reveal(ended); reveal(retirement::stopped); reveal(retirement::incident);
}

/// #860: resource monotonicity lifts the actor frame through the complete checker.
pub(super) proof fn world(b: World, a: World, generation: u64, bo: Outbox, ao: Outbox)
    requires retirement::domain(b), retirement::accepted(b), retirement::bounded(b),
        apply_post(b, a, generation, bo, ao),
    ensures retirement::domain(a), retirement::accepted(a),
        forall|i: int| 0 <= i < 2 ==> node_change(b.nodes@[i], a.nodes@[i], b.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, i),
{
    reveal(apply_post); reveal(reply_post); reveal(retirement::accepted); reveal(retirement::bounded);
    assert(a.nodes@ =~= b.nodes@.update(0, a.nodes@[0]));
    actor(b, b.nodes@[0], generation);
    retirement::node_frame(b, a, a.nodes@[0]);
    retirement::node_frame(b, a, a.nodes@[1]);
    assert forall|r: int| 0 <= r < 2 implies retirement::holders(a, RepoId(r as u64), 2) <= a.gates@[r].w by {
        assert(retirement::holders(a, RepoId(r as u64), 2) <= retirement::holders(b, RepoId(r as u64), 2));
    }
    footprint::storage_frame(b, a, b.nodes@[0].footprint, b.nodes@[1].footprint);
    assert forall|i: int| 0 <= i < 2 implies node_change(b.nodes@[i], a.nodes@[i], b.fail_fast,
        Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, i) by {
        assert(i == 0 || i == 1);
    }
}
}
