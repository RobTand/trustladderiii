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
pub(super) proof fn inactive_pair(before: World, off: World, on: World,
    generation: u64, reply: Reply, initial_out: Outbox, off_out: Outbox, on_out: Outbox)
    requires before.nodes@.len() == 2, !before.fail_fast,
        !(before.nodes@[0].phase is Stopping),
        !port_review_reply::active(before.nodes@[0], generation)
            || !port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial_out, off_out),
        port_review_reply::reply_post(World { fail_fast: true, ..before }, on,
            0, generation, reply, initial_out, on_out),
    ensures same_world(off, on),
        !port_review_reply::active(before.nodes@[0], generation)
            ==> off_out == initial_out && on_out == initial_out,
{
    reveal(port_review_reply::reply_post);
    let files = match reply {
        Reply::MakerNeedsFiles { files, .. } => files,
        Reply::PlanLeaf { footprint, .. } => footprint,
        _ => before.nodes@[0].footprint,
    };
    footprint::storage_frame(before, World { fail_fast: true, ..before }, files, files);
    assert(port_review_reply::accepted_reply(before, before.nodes@[0], reply)
        == port_review_reply::accepted_reply(World { fail_fast: true, ..before }, before.nodes@[0], reply));
    assert(off.nodes@ =~= on.nodes@);
}
}
