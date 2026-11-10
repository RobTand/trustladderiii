//! G5: Slot-arm node premises consumed by internal-safety composition.
use super::*;

verus! {
pub(super) open spec fn world_data_frame(w0: World, w1: World, t0: u64, t1: u64) -> bool {
    w0.gates@ == w1.gates@ && w0.file_sets@ == w1.file_sets@
        && w0.next_question == w1.next_question
        && w0.next_ticket == t0 && w1.next_ticket == t1
}

pub(super) open spec fn node_facts(b: Node, n: Node, t0: u64, t1: u64,
    c: AdvanceContext) -> bool {
    transition_safety::node_step(b, n)
        && n.adopted == b.adopted && n.repo == b.repo && n.footprint == b.footprint
        && n.head == b.head && n.approved == b.approved && n.checked == b.checked
        && n.paused == b.paused && n.last_cmd == b.last_cmd
        && t0 <= t1
        && (n.ticket == b.ticket || n.ticket.is_none()
            || (b.ticket.is_none() && n.ticket == Some(t0)))
        && (n.holds && !b.holds ==> c.slot && !b.paused
            && b.phase is Ready && b.ticket.is_some() && n.ticket.is_none())
        && (n.pool && !b.pool ==> c.pool && !b.paused
            && b.ticket.is_some() && n.ticket.is_none())
        && (n.credit.withheld ==> b.credit.withheld)
        && (dependencies::owner_waiting_ok(b) && !c.admit
            ==> dependencies::owner_waiting_ok(n))
}

pub(super) open spec fn local_outputs(b: Node, n: Node, c: AdvanceContext,
    o0: Outbox, o1: Outbox) -> bool {
    o0.len <= o1.len <= OUTBOX
        && (forall|k: int| o0.len <= k < o1.len ==> match #[trigger] o1.items@[k] {
            Some(Out::Effect { node, seq, kind, head }) => node == NodeId(c.node as u64)
                && b.next_seq <= seq < n.next_seq
                && (b.paused && !(b.phase is Stopping) ==> transition_safety::teardown_kind(kind))
                && (kind == EffectKind::Enqueue ==> match head {
                    Some(h) => n.approved == Some(h) && n.checked == Some(h),
                    None => false,
                }),
            _ => true,
        })
        && (forall|k: int| 0 <= k < OUTBOX && (k < o0.len || o1.len <= k)
            ==> o1.items@[k] == o0.items@[k])
}

pub(super) proof fn rules_slot(w0: World, w1: World,
    b: Node, n: Node, t0: u64, t1: u64, c: AdvanceContext,
    o0: Outbox, o1: Outbox, moved: bool)
    requires world_data_frame(w0, w1, t0, t1), retirement::node_rules(w0, b),
        advance_pre(b, t0, o0.len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        advance_family_post::slot(b, n, t0, t1, c, o0, o1, moved),
    ensures retirement::node_rules(w1, n), node_facts(b, n, t0, t1, c),
        local_outputs(b, n, c, o0, o1),
{
    reveal(advance_family_post::slot);
    reveal(retirement::node_rules);
    assert(node_checked(w0, b));
    assert(b.phase is Ready && b.pos == 0) by {
        reveal(current_item);
        reveal(items);
    }
    footprint::storage_frame(w0, w1, b.footprint, b.footprint);
    assert(!b.holds && !b.pool && b.run.is_none() && b.job.is_none()
        && b.question.is_none() && !b.credit.withheld);
    assert(!dependencies::owner_waiting_ok(b));
    match b.phase {
        Phase::Ready(r) => {
            assert(ready_checked(b, r));
            if b.paused {
                assert(n == (Node { ticket: None, ..b }) && t1 == t0);
                assert(node_checked(w1, n));
                assert(retirement::node_rules(w1, n));
            } else if b.ticket.is_none() {
                assert(n == (Node { ticket: Some(t0), ..b })
                    && t1 as int == t0 as int + 1);
                assert(t0 < w1.next_ticket);
                assert(node_checked(w1, n));
                assert(retirement::node_rules(w1, n));
            } else if c.slot {
                assert(n == retirement::phased(Node { holds: true, ..b }, resumed_phase(r)));
                assert(n.holds && n.ticket.is_none() && n.effect.is_none());
                assert(ready_checked(n, r));
                match r {
                    Resume::Making => { assert(node_checked(w1, n)); },
                    Resume::Checking => { assert(node_checked(w1, n)); },
                    Resume::Reviewing => { assert(node_checked(w1, n)); },
                    Resume::Publishing => { assert(node_checked(w1, n)); },
                    Resume::Rebasing => { assert(node_checked(w1, n)); },
                }
                assert(retirement::node_rules(w1, n));
            } else {
                assert(n == b && t1 == t0);
                assert(node_checked(w1, n));
                assert(retirement::node_rules(w1, n));
            }
        },
        _ => { assert(false); },
    }
    assert(node_facts(b, n, t0, t1, c));
    assert(o1.len == o0.len && o1.items@ == o0.items@);
    assert(local_outputs(b, n, c, o0, o1));
}

/// Consumer elimination lives outside the predicate's producer module.
pub(super) proof fn holds_from_frame(b: Node, a: Node)
    requires advance_family_post::holds_frame(b, a),
    ensures !b.paused && matches!(current_item(b), Some(Item::Do(_))) ==> a.holds == b.holds,
        matches!(current_item(b), Some(Item::Await | Item::Delivery | Item::Answer))
            ==> a.holds == b.holds,
{
    reveal(advance_family_post::holds_frame);
}


/// F2/R30: expose the literal Ready grant's phase and local frames.
pub(super) proof fn slot_resume_from_grant(b: Node, a: Node, c: AdvanceContext, changed: bool)
    requires advance_family_post::ready_unobstructed(b), changed,
        advance_family_post::slot_grant(b, a, c, changed),
    ensures a.phase == resumed_phase(match b.phase { Phase::Ready(r) => r, _ => Resume::Making }),
        a.pos == 0, !a.paused,
{
    reveal(advance_family_post::slot_grant);
}



/// E5.6 consumer elimination, outside the predicate's producer module.
pub(super) proof fn retained_holds(b: Node, a: Node)
    requires advance_family_post::retains_holds(b, a),
    ensures b.holds && !b.paused && (b.phase == Phase::Making || b.phase == Phase::Rebasing)
        ==> a.holds,
{
    reveal(advance_family_post::retains_holds);
}

}
