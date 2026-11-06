//! #806: compose review replies with the complete production checker.
use super::*;

verus! {
/// #806 check_nodes and Q54: exact reply branches preserve the common assertions.
pub(super) proof fn shape(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before),
        port_review_reply::selected(reply), before.next_question < u64::MAX,
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures after.next_question >= before.next_question,
        after.file_sets@ == before.file_sets@,
        port_review_safety::node_post(before.nodes@[0], after.nodes@[0], before.fail_fast),
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    let n = before.nodes@[0];
    if n.phase is Stopping || !port_review_reply::active(n, generation) {
        port_review_safety::inactive(n, after.nodes@[0], generation, before.fail_fast);
    } else if !port_review_reply::accepted_reply(before, n, reply) {
        reveal(retirement::accepted); reveal(retirement::node_rules);
        port_review_safety::rejected(n, after.nodes@[0], before.fail_fast);
    } else {
        reveal(port_review_reply::accepted_reply); reveal(port_review_reply::transition_post);
        reveal(retirement::accepted); reveal(retirement::node_rules);
        port_review_safety::accepted(n, after.nodes@[0], reply, before.fail_fast, before.next_question);
        reveal(port_review_reply::transition); reveal(port_review_retry::round);
        reveal(port_review_retry::rung); reveal(port_review_retry::room);
        reveal(port_review_retry::rechecked);
    }
}

/// State-table review/rebase rows: compose each handler's complete node rules.
pub(super) proof fn actor_rules(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before),
        port_review_reply::selected(reply), before.next_question < u64::MAX,
        node_headroom(before.nodes@[0]),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
        after.next_question >= before.next_question, after.file_sets@ == before.file_sets@,
    ensures retirement::node_rules(after, after.nodes@[0]),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(retirement::accepted);
    let n = before.nodes@[0];
    port_review_frames::question_frame(before, after, n);
    if n.phase is Stopping {
        reveal(retirement::node_rules);
    } else if !port_review_reply::active(n, generation) {
    } else if !port_review_reply::accepted_reply(before, n, reply) {
        port_review_frames::failed_rules(after, n);
    } else {
        reveal(port_review_reply::accepted_reply); reveal(port_review_reply::transition_post);
        match reply {
            Reply::Approved { head } => {
                port_review_frames::approved_rules(after, n, head);
                reveal(port_review_reply::transition);
            },
            Reply::ChangesRequested => port_review_frames::changes_rules(after, n, before.fail_fast, before.next_question),
            Reply::ReviewerOutOfRoom => port_review_frames::reviewer_rules(after, n, before.fail_fast, before.next_question),
            Reply::RebaseClean { head } => port_review_frames::rebase_clean_rules(after, n, before.fail_fast, before.next_question, head),
            Reply::RebaseNeedsChanges => port_review_frames::rebase_needs_rules(after, n, before.fail_fast, before.next_question),
            Reply::ChecksDone { .. } => port_review_frames::checks_rules(after, n, before.fail_fast, before.next_question, reply),
            _ => {},
        }
    }
}

/// #806 complete checker: replies release resources and never acquire a conflicting slot.
pub(super) proof fn world_rules(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before),
        port_review_reply::selected(reply), before.next_question < u64::MAX,
        node_headroom(before.nodes@[0]),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures retirement::domain(after), retirement::accepted(after),
        port_review_safety::node_post(before.nodes@[0], after.nodes@[0], before.fail_fast),
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
{
    shape(before, after, generation, reply, out_before, out_after);
    actor_rules(before, after, generation, reply, out_before, out_after);
    reveal(port_review_reply::reply_post); reveal(port_review_safety::node_post);
    reveal(retirement::accepted);
    port_review_frames::question_frame(before, after, after.nodes@[1]);
    assert forall|r: int| 0 <= r < 2 implies
        retirement::holders(after, RepoId(r as u64), 2) <= after.gates@[r].w by {
        assert(retirement::holders(after, RepoId(r as u64), 2)
            <= retirement::holders(before, RepoId(r as u64), 2));
    }
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}

kernel_fn! {
/// #806 review/checks slices: real apply and complete checker on a weaker domain than Kani's.
/// One event, no unwind. Counters may reach 2^40 and heads, gates and pool are unbounded.
pub(super) fn verify_apply(mut w: World, generation: u64, reply: Reply) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        storage_ordered(w), port_review_reply::selected(reply),
    ensures retirement::domain(result.0), retirement::accepted(result.0),
        retirement::notices_only(result.1),
        port_review_properties::specific_post(w, result.0, generation, reply),
        port_review_safety::node_post(w.nodes@[0], result.0.nodes@[0], w.fail_fast),
        result.0.nodes@[1] == w.nodes@[1],
        port_review_reply::reply_post(w, result.0, 0, generation, reply,
            Outbox { items: [None; OUTBOX], len: 0 }, result.1),
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(storage_ordered);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(retirement::accepted); reveal(retirement::bounded); reveal(retirement::notices_only);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        assert(world_headroom(w));
        dependencies::indexable_definition(w);
    }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    let mut out = Outbox::new();
    kernel_proof! {
        assert(out.items =~= [None; OUTBOX]);
        assert(out == Outbox { items: [None; OUTBOX], len: 0 });
    }
    kernel_snapshot! { before_outputs = out }
    apply(&mut w, Event::Reply { node: NodeId(0), generation, reply }, &mut out);
    kernel_proof! {
        world_rules(before@, w, generation, reply, before_outputs@, out);
        port_review_properties::specifics(before@, w, generation, reply, before_outputs@, out);
        reveal(port_review_reply::reply_post);
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out)
}
}
