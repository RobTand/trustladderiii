//! Consumer reveal of event_frames::out_of_range_stutter (form rule 2), in its own module.
use super::*;

verus! {

/// The out-of-range stutter rows, verbatim.
pub(super) proof fn out_of_range_stutter_rows(before: World, after: World, ob: Outbox, oa: Outbox, event: Event)
    requires event_frames::out_of_range_stutter(before, after, ob, oa, event),
    ensures match event {
        Event::Reply { node, .. } | Event::Fact { node, .. } | Event::Command { node, .. } =>
            node.0 >= before.nodes@.len() ==> after == before && oa == ob,
        _ => true,
    },
{
    reveal(event_frames::out_of_range_stutter);
}

}
