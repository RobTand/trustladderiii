//! R_T out-of-range stutter (MASTER-CURRENT item 6, owner apply): an event whose target is out of range leaves
//! the world and the outbox unchanged. One opaque clause on events::apply (form rule 2); consumers reveal it only
//! through event_frames_reveal. Ack's row is `true` here until debt's apply_ack carries the same post.
use super::*;

verus! {

#[verifier::opaque]
pub(super) open spec fn out_of_range_stutter(before: World, after: World, ob: Outbox, oa: Outbox, event: Event)
    -> bool {
    match event {
        Event::Reply { node, .. } | Event::Fact { node, .. } | Event::Command { node, .. } =>
            node.0 >= before.nodes@.len() ==> after == before && oa == ob,
        _ => true,
    }
}

/// Intro (events::apply ends with this call).
pub(super) proof fn out_of_range_stutter_intro(before: World, after: World, ob: Outbox, oa: Outbox, event: Event)
    requires match event {
        Event::Reply { node, .. } | Event::Fact { node, .. } | Event::Command { node, .. } =>
            node.0 >= before.nodes@.len() ==> after == before && oa == ob,
        _ => true,
    },
    ensures out_of_range_stutter(before, after, ob, oa, event),
{
    reveal(out_of_range_stutter);
}

}
