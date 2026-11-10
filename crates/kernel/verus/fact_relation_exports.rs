use super::*;

verus! {

#[verifier::opaque]
pub(super) open spec fn fact_rows(before: World, after: World, node: NodeId,
    fact: Fact, ob: Outbox, oa: Outbox) -> bool {
    fact is Priority && checking::accepted(before) && node.0 < before.nodes@.len()
        ==> checking::accepted(after)
            && !transition_safety::transition_safety(before, after,
                transition_safety::Transition::Apply(Event::Fact { node, fact }),
                transition_safety::emitted(ob, oa))
}

pub(super) proof fn fact_rows_intro(before: World, after: World, node: NodeId,
    fact: Fact, ob: Outbox, oa: Outbox)
    requires fact is Priority && checking::accepted(before) && node.0 < before.nodes@.len()
        ==> checking::accepted(after)
            && transition_safety::transition_safety(before, after,
                transition_safety::Transition::Apply(Event::Fact { node, fact }),
                transition_safety::emitted(ob, oa)),
    ensures fact_rows(before, after, node, fact, ob, oa),
{
    reveal(fact_rows);
}

}
