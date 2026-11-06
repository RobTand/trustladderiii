//! Q54 / #440 / #806: flag-independent reply admission for paired real worlds.
use super::*;

verus! {
/// Q54 paired-world check: every field compared by the real World's equality,
/// after normalizing fail_fast. Vector contents are compared through their views.
pub(super) open spec fn same_world(a: World, b: World) -> bool {
    a.nodes@ == b.nodes@ && a.gates@ == b.gates@
        && a.pool_capacity == b.pool_capacity && a.next_ticket == b.next_ticket
        && a.next_question == b.next_question && a.file_sets@ == b.file_sets@
        && a.dependencies@ == b.dependencies@
}

/// Q54 / #440: stale and schema-rejected replies do not divert. This composition
/// uses the shared real on_reply/apply relation for arbitrary generations/payloads.
/// The flag-on input is any world equal to `before` in every field except fail_fast.
pub(super) proof fn inactive_pair(before: World, fast: World, off: World, on: World,
    generation: u64, reply: Reply, initial_out: Outbox, off_out: Outbox, on_out: Outbox)
    requires before.nodes@.len() == 2, !before.fail_fast, fast.fail_fast, same_world(before, fast),
        !(before.nodes@[0].phase is Stopping),
        !port_review_reply::active(before.nodes@[0], generation)
            || !port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial_out, off_out),
        port_review_reply::reply_post(fast, on, 0, generation, reply, initial_out, on_out),
    ensures same_world(off, on),
        !port_review_reply::active(before.nodes@[0], generation)
            ==> off_out == initial_out && on_out == initial_out,
{
    reveal(port_review_reply::reply_post);
    reveal(port_review_reply::reply_result); reveal(port_review_reply::accepted_reply);
    let files = match reply {
        Reply::MakerNeedsFiles { files, .. } => files,
        Reply::PlanLeaf { footprint, .. } => footprint,
        _ => before.nodes@[0].footprint,
    };
    footprint::storage_frame(before, fast, files, files);
    assert(port_review_reply::accepted_reply(before, before.nodes@[0], reply)
        == port_review_reply::accepted_reply(fast, before.nodes@[0], reply));
    assert(off.nodes@ =~= on.nodes@);
}

/// Q30 / Q54: a stale reply preserves the complete production checker and
/// every shared node assertion, even when the two worlds use different flags.
pub(super) proof fn stale_world(before: World, after: World)
    requires retirement::domain(before), retirement::accepted(before),
        same_world(before, after),
    ensures retirement::domain(after), retirement::accepted(after),
        forall|i: int| 0 <= i < 2 ==>
            port_failfast_safety::node_post(before.nodes@[i], after.nodes@[i]),
{
    reveal(retirement::accepted);
    retirement::node_frame(before, after, before.nodes@[0]);
    retirement::node_frame(before, after, before.nodes@[1]);
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}

/// Q54 / #440 rejected maker-file replies: infrastructure recovery preserves
/// every complete node rule and never changes the work-retry ladder or credit.
pub(super) proof fn failure_node(w: World, n: Node)
    requires retirement::node_rules(w, n), node_headroom(n),
        n.phase == Phase::Making || n.phase is Planning,
        current_item(n) == Some(Item::Await),
    ensures retirement::node_rules(w, port_review_retry::failed_run(n)),
        port_review_retry::failed_run(n).credit == n.credit,
        port_review_retry::failed_run(n).review_stop.is_none(),
        port_review_retry::failed_run(n).attempt == n.attempt,
        port_review_retry::failed_run(n).replanned == n.replanned,
        port_review_retry::failed_run(n).question.is_none(),
        port_review_retry::failed_run(n).proposed == n.proposed,
        port_review_retry::failed_run(n).adopted == n.adopted,
        port_review_retry::failed_run(n).next_seq == n.next_seq,
{
    reveal(retirement::node_rules);
    reveal(port_review_retry::failed_run);
    reveal(retirement::incident);
}

/// Q54 rejected maker-file reply: compose the real failed-run result with
/// retirement's complete world predicate and its unchanged-storage frames.
pub(super) proof fn failure_world(before: World, after: World)
    requires retirement::domain(before), retirement::accepted(before),
        node_headroom(before.nodes@[0]),
        before.nodes@[0].phase == Phase::Making || before.nodes@[0].phase is Planning,
        current_item(before.nodes@[0]) == Some(Item::Await),
        after.nodes@.len() == 2, after.nodes@[1] == before.nodes@[1],
        after.nodes@[0] == port_review_retry::failed_run(before.nodes@[0]),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.pool_capacity == before.pool_capacity,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
    ensures retirement::domain(after), retirement::accepted(after),
        port_failfast_safety::node_post(before.nodes@[0], after.nodes@[0]),
        after.nodes@[0].credit == before.nodes@[0].credit,
        after.nodes@[0].review_stop.is_none(), after.nodes@[0].question.is_none(),
        after.nodes@[0].attempt == before.nodes@[0].attempt,
        after.nodes@[0].replanned == before.nodes@[0].replanned,
        after.nodes@[0].proposed == before.nodes@[0].proposed,
        after.nodes@[0].adopted == before.nodes@[0].adopted,
        after.nodes@[0].next_seq == before.nodes@[0].next_seq,
{
    reveal(retirement::accepted);
    failure_node(before, before.nodes@[0]);
    retirement::node_frame(before, after, after.nodes@[0]);
    retirement::node_frame(before, after, after.nodes@[1]);
    reveal(port_review_retry::failed_run);
    reveal(retirement::incident);
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}

}
