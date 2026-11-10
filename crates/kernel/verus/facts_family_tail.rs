//! Separately measured CI, outside-push and Fact output properties.
use super::*;
use facts_family_values::{ci_active, ci_prepared};
use facts_family_values::tail_values::*;

verus! {
pub(super) proof fn ci_prefix_frame(n: Node, infra: bool)
    ensures ci_prepared(n, infra).phase == n.phase,
        ci_prepared(n, infra).generation == n.generation,
        ci_prepared(n, infra).next_seq == n.next_seq,
        ci_prepared(n, infra).credit == n.credit,
        ci_prepared(n, infra).repo == n.repo,
        ci_prepared(n, infra).footprint == n.footprint,
        ci_prepared(n, infra).last_cmd == n.last_cmd,
        node_headroom(ci_prepared(n, infra)) == node_headroom(n),
        arithmetic_consistent(ci_prepared(n, infra)) == arithmetic_consistent(n),
        settle_shape(ci_prepared(n, infra)) == settle_shape(n),
        transition_safety::node_step(n, ci_prepared(n, infra)),
{
}

pub(super) proof fn ci_infrastructure_exact(n: Node, nq: u64, fast: bool)
    ensures reply_family_values::finish(nq, ci_change(n, fast, true))
        == retirement::fact_node(n, Fact::CiFailed { infra: true }),
        reply_family_values::next_question(nq, ci_change(n, fast, true)) == nq,
{
    reveal(ci_change);
    if ci_active(n, true) {
        let prepared = ci_prepared(n, true);
        let retry = retirement::infrastructure_retry(prepared);
        retirement::infrastructure_result(prepared,
            reply_family_values::finish(nq, ci_change(n, fast, true)), retry.1);
        facts_ci_failed_lemmas::cancellation_input(n, prepared);
    } else {
        reveal(retirement::fact_node);
    }
}

pub(super) proof fn ci_scalar_posts(n: Node, nq: u64, fast: bool, infra: bool)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::retry_bound(n, nq, ci_change(n, fast, infra)),
        reply_family_values::next_question(nq, ci_change(n, fast, infra)) as int
            <= nq as int + 1,
        n.phase is Closed ==> reply_family_values::finish(nq,
            ci_change(n, fast, infra)).phase is Closed,
        infra ==> reply_family_values::finish(nq, ci_change(n, fast, infra))
            == retirement::fact_node(n, Fact::CiFailed { infra: true }),
        infra ==> reply_family_values::next_question(nq, ci_change(n, fast, infra)) == nq,
        facts_family_values::tail_frame(n, reply_family_values::finish(nq, ci_change(n, fast, infra))),
{
    reveal(facts_family_values::tail_frame);
    ci_prefix_frame(n, infra);
    if infra { ci_infrastructure_exact(n, nq, fast); }
    reveal(ci_change);
    if ci_active(n, infra) {
        let prepared = ci_prepared(n, infra);
        if infra {
            reveal(retirement::infrastructure_retry);
            reveal(retirement::incident);
            reveal(retirement::fact_node);
        } else {
            reply_family_posts::failed_round_value_posts(prepared, fast, nq, ReviewStop::CiFailed);
        }
    }
}

pub(super) proof fn ci_node_step(n: Node, nq: u64, fast: bool, infra: bool)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures transition_safety::node_step(n,
        reply_family_values::finish(nq, ci_change(n, fast, infra))),
{
    if infra {
        ci_infrastructure_exact(n, nq, fast);
        apply_safety::selected_fact_step(n, Fact::CiFailed { infra: true });
    } else {
        ci_prefix_frame(n, infra);
        reveal(ci_change);
        if ci_active(n, infra) {
            let prepared = ci_prepared(n, infra);
            reply_family_posts::failed_round_value_posts(prepared, fast, nq, ReviewStop::CiFailed);
            transition_safety::node_step_trans(n, prepared,
                reply_family_values::finish(nq, ci_change(n, fast, infra)));
        }
    }
}

pub(super) proof fn original_review_input(origin: World, n: Node)
    requires retirement::node_rules(origin, n),
    ensures n.review_stop.is_some() ==> n.phase is Stopping || n.phase is Closed,
{
    reveal(retirement::node_rules);
}

pub(super) proof fn ci_fail_fast(n: Node, nq: u64, fast: bool, infra: bool, i: usize)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.review_stop.is_some() ==> n.phase is Stopping || n.phase is Closed,
    ensures transition_safety::fail_fast_node(n,
        reply_family_values::finish(nq, ci_change(n, fast, infra)), fast,
        Event::Fact { node: NodeId(i as u64), fact: Fact::CiFailed { infra } }, i as int),
{
    reveal(ci_change);
    if ci_active(n, infra) {
        if infra {
            ci_infrastructure_exact(n, nq, fast);
            reveal(retirement::fact_node);
        } else {
            reveal(reply_family_values::failed_round);
            if n.round != 0 {
                reveal(reply_family_values::rung);
                if fast { reveal(retirement::stopped); }
            }
        }
    }
}

pub(super) proof fn ci_node_rows(n: Node, nq: u64, fast: bool, infra: bool, i: usize)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures facts_family_values::node_rows(n,
        reply_family_values::finish(nq, ci_change(n, fast, infra)), fast, i,
        Fact::CiFailed { infra }),
{
    ci_node_step(n, nq, fast, infra);
    assert forall|origin: World| #[trigger] retirement::node_rules(origin, n)
        implies transition_safety::fail_fast_node(n,
            reply_family_values::finish(nq, ci_change(n, fast, infra)), fast,
            Event::Fact { node: NodeId(i as u64), fact: Fact::CiFailed { infra } }, i as int) by {
        original_review_input(origin, n);
        ci_fail_fast(n, nq, fast, infra, i);
    }
    reveal(facts_family_values::node_rows);
}

pub(super) proof fn outside_prefix_frame(n: Node, head: HeadId)
    ensures node_headroom(outside_prepared(n, head)) == node_headroom(n),
        arithmetic_consistent(outside_prepared(n, head)) == arithmetic_consistent(n),
        settle_shape(outside_prepared(n, head)) == settle_shape(n),
        transition_safety::node_step(n, outside_prepared(n, head)),
{
}

pub(super) proof fn outside_scalar_posts(n: Node, nq: u64, head: HeadId)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::retry_bound(n, nq, outside_change(n, head)),
        reply_family_values::next_question(nq, outside_change(n, head)) as int
            <= nq as int + 1,
        n.phase is Closed ==> reply_family_values::finish(nq,
            outside_change(n, head)).phase is Closed,
        facts_family_values::tail_frame(n, reply_family_values::finish(nq, outside_change(n, head))),
{
    reveal(facts_family_values::tail_frame);
    outside_prefix_frame(n, head);
    reveal(outside_change);
    if outside_active(n) {
        reply_family_posts::recheck_value_posts(outside_prepared(n, head), nq,
            Phase::Ready(Resume::Checking));
    }
}

pub(super) proof fn outside_node_step(n: Node, nq: u64, head: HeadId)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures transition_safety::node_step(n,
        reply_family_values::finish(nq, outside_change(n, head))),
{
    outside_prefix_frame(n, head);
    reveal(outside_change);
    if outside_active(n) {
        let prepared = outside_prepared(n, head);
        reply_family_posts::recheck_value_posts(prepared, nq, Phase::Ready(Resume::Checking));
        transition_safety::node_step_trans(n, prepared,
            reply_family_values::finish(nq, outside_change(n, head)));
    }
}

pub(super) proof fn outside_fail_fast(n: Node, nq: u64, fast: bool, head: HeadId, i: usize)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures transition_safety::fail_fast_node(n,
        reply_family_values::finish(nq, outside_change(n, head)), fast,
        Event::Fact { node: NodeId(i as u64), fact: Fact::PushByOther { head } }, i as int),
{
    reveal(outside_change);
    if outside_active(n) { reveal(reply_family_values::recheck); }
}

pub(super) proof fn outside_node_rows(n: Node, nq: u64, fast: bool, head: HeadId, i: usize)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures facts_family_values::node_rows(n,
        reply_family_values::finish(nq, outside_change(n, head)), fast, i,
        Fact::PushByOther { head }),
{
    outside_node_step(n, nq, head);
    outside_fail_fast(n, nq, fast, head, i);
    reveal(facts_family_values::node_rows);
}

pub(super) proof fn frame_fields(b: Node, n: Node)
    requires facts_family_values::tail_frame(b, n),
    ensures n.repo == b.repo, n.footprint == b.footprint,
        (settle_shape(b) ==> settle_shape(n)),
        n.generation as int <= b.generation as int + 1,
        n.next_seq == b.next_seq, n.credit.start == b.credit.start,
        n.credit.earned == b.credit.earned, n.credit.added == b.credit.added,
        n.credit.spent as int <= b.credit.spent as int + 1, n.last_cmd == b.last_cmd,
{
    reveal(facts_family_values::tail_frame);
}

pub(super) proof fn failed_round_frame(n: Node, nq: u64, fast: bool, step: ReviewStop)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures facts_family_values::tail_frame(n,
        reply_family_values::finish(nq, reply_family_values::failed_round(n, fast, step))),
        reply_family_values::next_question(nq, reply_family_values::failed_round(n, fast, step))
            as int <= nq as int + 1,
{
    reply_family_posts::failed_round_value_posts(n, fast, nq, step);
    reveal(facts_family_values::tail_frame);
    reveal(reply_family_values::failed_round);
    if n.round != 0 { reveal(reply_family_values::rung); }
}

pub(super) proof fn reply_notice_bound(notes: Outbox, nq: u64, change: ReplyChange, i: usize)
    requires notes.len == match reply_family_values::item(nq, change) { Some(_) => 1usize, None => 0usize },
        notes.items@ =~= facts_reply_adapter::reply_items(nq, change, i),
    ensures facts_family_values::notices(notes, i, 1),
{
    reveal(facts_family_values::notices);
}

pub(super) proof fn append_prefix(ob: Outbox, oa: Outbox, notes: Outbox)
    requires facts_family_values::appended(ob, oa, notes),
    ensures facts_family_values::prefix_preserved(ob, oa),
{
    reveal(facts_family_values::appended);
    reveal(facts_family_values::prefix_preserved);
}

pub(super) proof fn dependency_rows(before: World, after: World, i: usize, fact: Fact)
    requires i < before.nodes@.len(), dependencies::dependency_frame(before, after, i as int),
        fact is Dependency || fact is DependencyRemove,
    ensures facts_family_values::node_rows(before.nodes@[i as int], after.nodes@[i as int],
        before.fail_fast, i, fact),
{
    reveal(dependencies::dependency_frame);
    reveal(facts_family_values::node_rows);
}

pub(super) proof fn node_safety_rows(b: Node, n: Node, fast: bool, i: usize, fact: Fact,
    ob: Outbox, oa: Outbox, origin: World)
    requires facts_family_values::node_rows(b, n, fast, i, fact),
        facts_family_values::notice_suffix(ob, oa, i), retirement::node_rules(origin, b),
    ensures transition_safety::node_safety(b, n, fast,
        Event::Fact { node: NodeId(i as u64), fact }, i as int, ob, oa),
{
    reveal(facts_family_values::node_rows);
    reveal(facts_family_values::notice_suffix);
    reveal(transition_safety::node_safety);
}
}
