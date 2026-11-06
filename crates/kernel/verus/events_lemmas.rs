//! #771 / #805: isolate dispatcher input and footprint composition proofs.
use super::*;

verus! {
/// #805: an exact selector input supplies the real handler's local progress premise.
pub(super) proof fn selected_input(w: World, event: Event)
    ensures progress_input(w, event) ==> match event {
        Event::Reply { node, generation, reply } => node.0 < w.nodes@.len()
            && node_checked(w, w.nodes@[node.0 as int])
            && generation == w.nodes@[node.0 as int].generation
            && successful_reply_input(w.nodes@[node.0 as int], reply),
        Event::Ack { node, seq, ack } => node.0 < w.nodes@.len()
            && node_checked(w, w.nodes@[node.0 as int])
            && (!w.nodes@[node.0 as int].paused || w.nodes@[node.0 as int].phase is Stopping)
            && ack == Ack::Confirmed
            && (match w.nodes@[node.0 as int].effect { Some(e) => e.seq == seq, None => false }),
        Event::Command { node, id, command } => node.0 < w.nodes@.len()
            && node_checked(w, w.nodes@[node.0 as int])
            && id > w.nodes@[node.0 as int].last_cmd
            && command_progress_input(w.nodes@[node.0 as int], command),
        Event::Fact { node, fact } => node.0 < w.nodes@.len()
            && fact_progress_input(w, node.0 as int, fact),
        _ => false,
    },
{
    reveal(progress_input);
    reveal(fresh_commands);
    if progress_input(w, event) {
        world_checked_definition(w);
        let j = choose|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event);
        assert(j <= u64::MAX);
        reveal(fact_progress_input);
    }
}

/// #771: unchanged footprints survive table growth; newly appended nodes are empty.
pub(super) proof fn footprints_after(before: World, after: World)
    requires before.nodes@.len() <= after.nodes@.len(), footprint::ordered_table(after),
        after.file_sets@.len() >= before.file_sets@.len(),
        forall|j: int| 0 <= j < before.file_sets@.len()
            ==> after.file_sets@[j] == before.file_sets@[j],
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> footprint::valid_footprint(before, before.nodes@[j].footprint),
        forall|j: int| 0 <= j < after.nodes@.len() ==> if j < before.nodes@.len() {
            after.nodes@[j].footprint == before.nodes@[j].footprint
                || footprint::valid_footprint(after, after.nodes@[j].footprint)
        } else { after.nodes@[j].footprint == footprint::inline_footprint(0) },
    ensures forall|j: int| 0 <= j < after.nodes@.len()
        ==> footprint::valid_footprint(after, after.nodes@[j].footprint),
{
    assert forall|j: int| 0 <= j < after.nodes@.len() implies
        footprint::valid_footprint(after, after.nodes@[j].footprint) by {
        if j < before.nodes@.len() {
            if after.nodes@[j].footprint == before.nodes@[j].footprint {
                footprint::valid_under_table_growth(before, after, after.nodes@[j].footprint);
            }
        } else {
            footprint::inline_is_valid(after, 0);
        }
    }
}
}
