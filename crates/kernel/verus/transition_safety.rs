//! K4b #910: one opaque safety relation for apply, advance and settle.
//! Callers name the transition with a proof-only tag and consume narrow
//! projections. Only this module reveals the relation. Each concept predicate
//! restates one Kani assertion function from proofs.rs over arbitrary worlds:
//! the node index is any index, not node 0, and no node-count bound applies.
use super::*;

verus! {

/// K4b: proof-only tag for the transition a relation instance describes.
pub(super) enum Transition {
    Apply(Event),
    Advance { node: usize, moved: bool },
    Settle,
}

/// K4b: the output suffix a transition appends after the caller's prefix.
pub(super) open spec fn emitted(before: Outbox, after: Outbox) -> Seq<Option<Out>> {
    after.items@.subrange(before.len as int, after.len as int)
}

/// proofs.rs check_nodes: identity, effect-key, finality and merge-credit
/// rules for one node across one transition.
pub(super) open spec fn node_step(b: Node, n: Node) -> bool {
    n.proposed == b.proposed
        && (b.adopted ==> n.adopted)
        && n.next_seq >= b.next_seq
        && (b.phase is Closed ==> n.phase == b.phase
            || (n.phase == Phase::Closed(Outcome::Merged)
                && matches!(b.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced))))
        && n.credit.earned as int <= b.credit.earned as int + 1
        && (n.credit.earned > b.credit.earned
            ==> !b.credit.merge_credited && n.credit.merge_credited)
        && (b.credit.merge_credited ==> n.credit.merge_credited)
}

/// proofs.rs check_nodes over every node present before the transition.
pub(super) open spec fn nodes_step(before: World, after: World) -> bool {
    before.nodes@.len() <= after.nodes@.len()
        && forall|j: int| 0 <= j < before.nodes@.len()
            ==> node_step(before.nodes@[j], #[trigger] after.nodes@[j])
}

/// proofs.rs teardown: the effect kinds a paused node may still start.
pub(super) open spec fn teardown_kind(kind: EffectKind) -> bool {
    matches!(kind, EffectKind::StopRun | EffectKind::WithdrawJob
        | EffectKind::ClosePr | EffectKind::RemoveWorktree)
}

/// proofs.rs check_effect for an effect emitted by node i, which is n after
/// the transition. A node created by the transition has no before state.
pub(super) open spec fn effect_ok(before: World, n: Node, i: int, seq: u64,
    kind: EffectKind, head: Option<HeadId>, exempt: bool) -> bool {
    seq < n.next_seq
        && (0 <= i < before.nodes@.len() ==> {
            let b = before.nodes@[i];
            seq >= b.next_seq
                && (b.paused && !(b.phase is Stopping) && !exempt ==> teardown_kind(kind))
        })
        && (kind == EffectKind::Enqueue ==> match head {
            Some(h) => n.approved == Some(h) && n.checked == Some(h),
            None => false,
        })
}

/// proofs.rs check_outputs over the emitted suffix. `exempt` names the node
/// allowed to start a non-teardown effect while paused.
pub(super) open spec fn outputs_ok(before: World, after: World,
    out: Seq<Option<Out>>, exempt: Option<int>) -> bool {
    forall|k: int| 0 <= k < out.len() ==> match #[trigger] out[k] {
        Some(Out::Effect { node, seq, kind, head }) => node.0 < after.nodes@.len()
            && effect_ok(before, after.nodes@[node.0 as int], node.0 as int, seq, kind, head,
                exempt == Some(node.0 as int)),
        _ => true,
    }
}

/// check_apply: the node that may emit while paused, an acknowledged effect
/// or a resume.
pub(super) open spec fn apply_exempt(event: Event) -> Option<int> {
    match event {
        Event::Ack { node, .. } => Some(node.0 as int),
        Event::Command { node, command: Command::Resume, .. } => Some(node.0 as int),
        _ => None,
    }
}

/// check_fail_fast: an OpenPr body-policy refusal of node i's live effect.
pub(super) open spec fn publication_body(b: Node, event: Event, i: int) -> bool {
    match event {
        Event::Ack { node, seq, ack: Ack::BodyPolicyRefused } => node.0 == i
            && match b.effect {
                Some(e) => e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr,
                None => false,
            },
        _ => false,
    }
}

/// Q54 / #629 / Q27 #832, #832-A: the three permitted flag-off review stops.
/// The plan-check case is code-owned and never includes worker PlanCantMeet.
pub(super) open spec fn permitted_stop(b: Node, n: Node, event: Event, i: int) -> bool {
    match event {
        Event::Ack { .. } => publication_body(b, event, i)
            && n.review_stop == Some(ReviewStop::PublicationBody),
        Event::Reply { node, generation, reply: Reply::AuditDisagrees } => node.0 == i
            && b.phase == Phase::Auditing && b.audit_rounds >= 1
            && current_item(b) == Some(Item::Await) && b.run == Some(generation)
            && n.review_stop == Some(ReviewStop::AuditDisagreement),
        Event::Reply { node, generation, reply: Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit } =>
            node.0 == i && b.phase is Planning
                && current_item(b) == Some(Item::Await) && b.run == Some(generation)
                && n.review_stop == Some(ReviewStop::PlanCantMeet),
        _ => false,
    }
}

/// check_fail_fast: Rob's unclear answer re-asks the same open question.
pub(super) open spec fn clarifies(b: Node, p: Question, q: Question) -> bool {
    b.phase == Phase::AwaitingRob && p.kind == q.kind && p.cont == q.cont
        && p.cost == q.cost && !p.clarified && q.clarified
}

/// check_fail_fast: the transition asks a new can't-meet question.
pub(super) open spec fn asks_cant_meet(b: Node, n: Node) -> bool {
    match n.question {
        Some(q) => q.kind == QKind::CantMeet && match b.question {
            Some(p) => p.id != q.id && !clarifies(b, p, q),
            None => true,
        },
        None => false,
    }
}

/// check_fail_fast (Q54) for one node, with the three-case flag-off rule.
pub(super) open spec fn fail_fast_node(b: Node, n: Node, fast: bool, event: Event, i: int) -> bool {
    let stopped = n.review_stop != b.review_stop;
    (publication_body(b, event, i) ==> n.review_stop == Some(ReviewStop::PublicationBody))
        && (!fast ==> !stopped || permitted_stop(b, n, event, i))
        && (fast ==> n.attempt <= (if b.attempt > 1 { b.attempt } else { 1u8 }))
        && (fast ==> b.replanned || !n.replanned)
        && (fast && n.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && (fast && asks_cant_meet(b, n) ==> match event {
            Event::Reply { reply: Reply::PlanLeaf { .. }, .. } => true,
            _ => false,
        })
        && (stopped ==> b.review_stop.is_none()
            && n.phase == Phase::Stopping(Outcome::Stopped)
            && n.credit == b.credit && n.question.is_none())
}

/// check_fail_fast over every node present before the event.
pub(super) open spec fn fail_fast_step(before: World, after: World, event: Event) -> bool {
    forall|j: int| 0 <= j < before.nodes@.len()
        ==> fail_fast_node(before.nodes@[j], #[trigger] after.nodes@[j], before.fail_fast, event, j)
}

/// apply's existing world frame: configuration, tickets and file sets are
/// unchanged or extended, at most one node and one question are created, and
/// every existing node keeps its repository.
pub(super) open spec fn apply_frame(before: World, after: World) -> bool {
    before.nodes@.len() <= after.nodes@.len() <= before.nodes@.len() + 1
        && after.next_ticket == before.next_ticket
        && after.next_question as int <= before.next_question as int + 1
        && after.gates@ == before.gates@
        && after.pool_capacity == before.pool_capacity
        && after.fail_fast == before.fail_fast
        && after.file_sets@.len() >= before.file_sets@.len()
        && (forall|j: int| 0 <= j < before.file_sets@.len()
            ==> #[trigger] after.file_sets@[j] == before.file_sets@[j])
        && (forall|j: int| 0 <= j < before.nodes@.len()
            ==> (#[trigger] after.nodes@[j]).repo == before.nodes@[j].repo)
}

/// check_advance: a moving advance changes only its node and keeps the node
/// count; a false advance leaves the world and the output unchanged.
pub(super) open spec fn advance_frame(before: World, after: World, i: int, moved: bool,
    out: Seq<Option<Out>>) -> bool {
    if moved {
        0 <= i < before.nodes@.len() && settle_frame(before, after)
            && forall|j: int| 0 <= j < before.nodes@.len() && j != i
                ==> #[trigger] after.nodes@[j] == before.nodes@[j]
    } else {
        after == before && out.len() == 0
    }
}

/// K4b settle summary of every emitted effect: a valid node index and a key
/// from that node's interval across the whole settle. Per-emission paused and
/// Enqueue-head validity is each pass's Advance instance.
pub(super) open spec fn settle_item(before: World, after: World, o: Option<Out>) -> bool {
    match o {
        Some(Out::Effect { node, seq, .. }) => node.0 < after.nodes@.len()
            && seq < after.nodes@[node.0 as int].next_seq
            && (node.0 < before.nodes@.len() ==> seq >= before.nodes@[node.0 as int].next_seq),
        _ => true,
    }
}

/// K4b: settle_item over the whole emitted suffix.
pub(super) open spec fn settle_outputs(before: World, after: World, out: Seq<Option<Out>>) -> bool {
    forall|k: int| 0 <= k < out.len() ==> settle_item(before, after, #[trigger] out[k])
}

/// K4 REPLAN settle frame: current settle keeps the node count, dependency
/// table, gates, file sets, pool capacity, fail_fast and next question.
pub(super) open spec fn settle_frame(before: World, after: World) -> bool {
    after.nodes@.len() == before.nodes@.len()
        && after.dependencies@ == before.dependencies@
        && after.gates@ == before.gates@
        && after.file_sets@ == before.file_sets@
        && after.pool_capacity == before.pool_capacity
        && after.fail_fast == before.fail_fast
        && after.next_question == before.next_question
}

/// check_apply event rows, generalized from node 0 to the event's node.
pub(super) open spec fn event_rules(before: World, after: World, event: Event) -> bool {
    match event {
        Event::NewAdoption { repo, head, .. } =>
            (repo.0 < before.gates@.len() ==> after.nodes@.len() == before.nodes@.len() + 1
                && after.nodes@[before.nodes@.len() as int].proposed == Some(head)
                && !after.nodes@[before.nodes@.len() as int].adopted),
        Event::Reply { node, generation, reply } => node.0 < before.nodes@.len()
            ==> reply_rules(before.nodes@[node.0 as int], after.nodes@[node.0 as int],
                before.fail_fast, generation, reply),
        Event::Fact { node, fact: Fact::Conflict } => node.0 < before.nodes@.len() ==> {
            let b = before.nodes@[node.0 as int];
            let n = after.nodes@[node.0 as int];
            if b.phase == Phase::AwaitingDelivery || b.phase == Phase::Publishing {
                n.phase == (if b.phase == Phase::AwaitingDelivery {
                    Phase::Ready(Resume::Rebasing) } else { Phase::Rebasing })
                    && n.generation == b.generation + 1
                    && n.credit == b.credit && n.holds == b.holds
            } else { n == b }
        },
        Event::Command { node, command: Command::Answer { question, .. }, .. } =>
            node.0 < before.nodes@.len() ==> {
                let b = before.nodes@[node.0 as int];
                let n = after.nodes@[node.0 as int];
                n != Node { last_cmd: n.last_cmd, ..b } ==> match b.question {
                    Some(q) => q.id == question,
                    None => false,
                } && match n.question {
                    Some(q) => q.id != question,
                    None => true,
                }
            },
        _ => true,
    }
}

/// check_apply: completed catch-up consumes finite free allowance or credit.
pub(super) open spec fn rebase_fuel(n: Node) -> int {
    (if n.credit.withheld { 0int } else {
        n.credit.start as int + n.credit.earned as int + n.credit.added as int - n.credit.spent as int
    }) + (if n.rechecks < FREE_RECHECKS { (FREE_RECHECKS - n.rechecks) as int } else { 0int })
}

/// The accepted ownership of a reply in check_apply's event rows.
pub(super) open spec fn owns(b: Node, generation: u64) -> bool {
    current_item(b) == Some(Item::Await) && b.run == Some(generation)
}

/// check_apply, check_fail_fast and check_fail_fast_funding reply rows.
pub(super) open spec fn reply_rules(b: Node, n: Node, fast: bool, generation: u64, reply: Reply) -> bool {
    match reply {
        Reply::ProtectedPaths => (matches!(b.phase,
                Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }) && owns(b, generation))
            ==> n.phase == Phase::Stopping(Outcome::NotActionable) && n.credit == b.credit
                && n.question.is_none() && n.plan_retries == b.plan_retries,
        Reply::ReviewerOutOfRoom => (b.phase == Phase::Reviewing && owns(b, generation)) ==> {
            let can_spend = !b.credit.withheld && b.credit.start as int + b.credit.earned as int
                + b.credit.added as int >= b.credit.spent as int + 1;
            let rechecks = if b.rechecks < 255 { (b.rechecks + 1) as u8 } else { 255u8 };
            n.run_failures == 0 && n.attempt == b.attempt && n.round == b.round
                && if b.review_restarted && fast {
                    n.phase == Phase::Stopping(Outcome::Stopped)
                        && n.review_stop == Some(ReviewStop::ReviewerOutOfRoom) && n.credit == b.credit
                } else if b.review_restarted {
                    !n.holds && if can_spend {
                        n.phase == Phase::Planning { must_split: true }
                            && n.credit.spent == b.credit.spent + 1
                    } else {
                        n.phase == Phase::AwaitingRob && match n.question {
                            Some(q) => q.cont == Cont::Planning { must_split: true },
                            None => false,
                        }
                    }
                } else {
                    n.review_restarted && n.rechecks == rechecks && n.checked == b.checked
                        && if n.rechecks <= FREE_RECHECKS || can_spend {
                            n.phase == Phase::Reviewing && n.generation > b.generation
                                && n.credit.spent == b.credit.spent
                                    + (if n.rechecks > FREE_RECHECKS { 1u64 } else { 0u64 })
                        } else {
                            n.phase == Phase::AwaitingRob && match n.question {
                                Some(q) => q.cont == Cont::Ready(Resume::Reviewing),
                                None => false,
                            }
                        }
                }
        },
        Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges =>
            (b.phase == Phase::Rebasing && owns(b, generation))
                ==> n.phase == Phase::AwaitingRob || rebase_fuel(n) < rebase_fuel(b),
        Reply::PlanCantMeet => (fast && b.phase is Planning && owns(b, generation))
            ==> n.phase == Phase::Stopping(Outcome::Stopped)
                && n.review_stop == Some(ReviewStop::PlanCantMeet),
        _ => true,
    }
}

/// K4b #910: the common transition-safety relation. The Apply arm is every
/// check_apply assertion that does not need the complete checker; the
/// checker-preservation conjunct joins when the checker export qualifies.
#[verifier::opaque]
pub(super) open spec fn transition_safety(before: World, after: World, t: Transition,
    out: Seq<Option<Out>>) -> bool {
    match t {
        Transition::Apply(event) => apply_frame(before, after)
            && nodes_step(before, after)
            && outputs_ok(before, after, out, apply_exempt(event))
            && fail_fast_step(before, after, event)
            && event_rules(before, after, event),
        Transition::Advance { node, moved } => advance_frame(before, after, node as int, moved, out)
            && (moved ==> nodes_step(before, after) && outputs_ok(before, after, out, None)),
        Transition::Settle => settle_frame(before, after) && nodes_step(before, after)
            && settle_outputs(before, after, out),
    }
}

/// K4b: establish the relation from its concept conjuncts.
pub(super) proof fn intro(before: World, after: World, t: Transition, out: Seq<Option<Out>>)
    requires match t {
        Transition::Apply(event) => apply_frame(before, after)
            && nodes_step(before, after)
            && outputs_ok(before, after, out, apply_exempt(event))
            && fail_fast_step(before, after, event)
            && event_rules(before, after, event),
        Transition::Advance { node, moved } => advance_frame(before, after, node as int, moved, out)
            && (moved ==> nodes_step(before, after) && outputs_ok(before, after, out, None)),
        Transition::Settle => settle_frame(before, after) && nodes_step(before, after)
            && settle_outputs(before, after, out),
    },
    ensures transition_safety(before, after, t, out),
{
    reveal(transition_safety);
}

/// K4b: the field-group frame of each transition kind.
pub(super) proof fn frame(before: World, after: World, t: Transition, out: Seq<Option<Out>>)
    requires transition_safety(before, after, t, out),
    ensures match t {
        Transition::Apply(_) => apply_frame(before, after),
        Transition::Advance { node, moved } => advance_frame(before, after, node as int, moved, out),
        Transition::Settle => settle_frame(before, after),
    },
{
    reveal(transition_safety);
}

/// K4b: check_nodes for apply, a moving advance and a whole settle.
pub(super) proof fn nodes(before: World, after: World, t: Transition, out: Seq<Option<Out>>)
    requires transition_safety(before, after, t, out),
        match t { Transition::Advance { moved, .. } => moved, _ => true },
    ensures nodes_step(before, after),
{
    reveal(transition_safety);
}

/// K4b: check_outputs for apply and for a moving advance.
pub(super) proof fn outputs(before: World, after: World, t: Transition, out: Seq<Option<Out>>)
    requires transition_safety(before, after, t, out),
        match t { Transition::Apply(_) => true, Transition::Advance { moved, .. } => moved,
            Transition::Settle => false },
    ensures outputs_ok(before, after, out, match t {
            Transition::Apply(event) => apply_exempt(event), _ => None }),
{
    reveal(transition_safety);
}

/// K4b: settle's emitted outputs are the concatenation of its passes' suffixes.
/// Outboxes only append, so a later outbox keeps every earlier slot.
pub(super) proof fn emitted_concat(o0: Outbox, o1: Outbox, o2: Outbox)
    requires o0.len <= o1.len <= o2.len <= OUTBOX,
        forall|k: int| 0 <= k < o1.len ==> #[trigger] o2.items@[k] == o1.items@[k],
    ensures emitted(o0, o2) == emitted(o0, o1) + emitted(o1, o2),
{
    assert(o1.items@.len() == OUTBOX && o2.items@.len() == OUTBOX);
    assert(emitted(o0, o2) =~= emitted(o0, o1) + emitted(o1, o2));
}

/// K4b: a settle that has taken no pass yet.
pub(super) proof fn settle_start(w: World)
    ensures transition_safety(w, w, Transition::Settle, Seq::empty()),
{
    reveal(transition_safety);
    assert forall|j: int| 0 <= j < w.nodes@.len()
        implies node_step(w.nodes@[j], #[trigger] w.nodes@[j]) by {}
}

/// K4b: extend a settle by one internal advance, moving or stuttering, and
/// concatenate the emitted suffixes.
pub(super) proof fn transition_compose(w0: World, w1: World, w2: World, node: usize, moved: bool,
    first: Seq<Option<Out>>, next: Seq<Option<Out>>)
    requires transition_safety(w0, w1, Transition::Settle, first),
        transition_safety(w1, w2, Transition::Advance { node, moved }, next),
    ensures transition_safety(w0, w2, Transition::Settle, first + next),
{
    reveal(transition_safety);
    if moved {
        nodes_step_trans(w0, w1, w2);
        let out = first + next;
        assert forall|k: int| 0 <= k < out.len() implies settle_item(w0, w2, #[trigger] out[k]) by {
            if k < first.len() {
                assert(out[k] == first[k]);
                assert(settle_item(w0, w1, first[k]));
            } else {
                assert(out[k] == next[k - first.len()]);
                assert(match next[k - first.len()] {
                    Some(Out::Effect { node, seq, kind, head }) => node.0 < w2.nodes@.len()
                        && effect_ok(w1, w2.nodes@[node.0 as int], node.0 as int, seq, kind, head,
                            false),
                    _ => true,
                });
            }
        }
    } else {
        assert(first + next =~= first);
    }
}

/// K4b: the emitted-effect summary of a whole settle.
pub(super) proof fn settle_effects(before: World, after: World, out: Seq<Option<Out>>)
    requires transition_safety(before, after, Transition::Settle, out),
    ensures settle_outputs(before, after, out),
{
    reveal(transition_safety);
}

/// K4b: check_nodes composes, because merge credit is earned once and kept.
pub(super) proof fn node_step_trans(a: Node, b: Node, c: Node)
    requires node_step(a, b), node_step(b, c),
    ensures node_step(a, c),
{
}

/// K4b: check_nodes over every node composes across two transitions.
pub(super) proof fn nodes_step_trans(w0: World, w1: World, w2: World)
    requires nodes_step(w0, w1), nodes_step(w1, w2),
    ensures nodes_step(w0, w2),
{
    assert forall|j: int| 0 <= j < w0.nodes@.len()
        implies node_step(w0.nodes@[j], #[trigger] w2.nodes@[j]) by {
        node_step_trans(w0.nodes@[j], w1.nodes@[j], w2.nodes@[j]);
    }
}

/// K4b: Q54 check_fail_fast and the check_apply event rows for one apply.
pub(super) proof fn apply_rules(before: World, after: World, event: Event, out: Seq<Option<Out>>)
    requires transition_safety(before, after, Transition::Apply(event), out),
    ensures fail_fast_step(before, after, event), event_rules(before, after, event),
{
    reveal(transition_safety);
}

}
