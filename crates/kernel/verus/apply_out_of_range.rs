//! An event whose target is absent leaves the world and output unchanged.
use super::*;

verus! {

/// The Apply relation for every targeted event outside the node table.
/// This stutter property holds without an acceptance premise.
pub(super) proof fn apply_out_of_range(w: World, event: Event)
    requires match event {
        Event::Reply { node, .. } | Event::Ack { node, .. }
        | Event::Fact { node, .. } | Event::Command { node, .. }
            => node.0 >= w.nodes@.len(),
        Event::NewRequest { .. } | Event::NewAdoption { .. } => false,
    },
    ensures transition_safety::transition_safety(w, w,
        transition_safety::Transition::Apply(event), Seq::empty()),
{
    assert forall|j: int| 0 <= j < w.nodes@.len()
        implies transition_safety::node_step(w.nodes@[j], #[trigger] w.nodes@[j])
            && transition_safety::fail_fast_node(w.nodes@[j], w.nodes@[j],
                w.fail_fast, event, j) by {
        transition_safety::unchanged_node(w.nodes@[j], w.fail_fast, event, j);
    }
    transition_safety::intro(w, w, transition_safety::Transition::Apply(event), Seq::empty());
}

}
