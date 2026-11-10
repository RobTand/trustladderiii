use super::*;

verus! {

pub(super) proof fn fact_rows(before: World, after: World, node: NodeId,
    fact: Fact, ob: Outbox, oa: Outbox)
    requires fact_relation_exports::fact_rows(before, after, node, fact, ob, oa),
    ensures fact is Priority && checking::accepted(before) && node.0 < before.nodes@.len()
        ==> checking::accepted(after)
            && transition_safety::transition_safety(before, after,
                transition_safety::Transition::Apply(Event::Fact { node, fact }),
                transition_safety::emitted(ob, oa)),
{
    reveal(fact_relation_exports::fact_rows);
}

pub(super) proof fn priority_model(b: Node, priority: Priority)
    ensures retirement::fact_node(b, Fact::Priority(priority))
        == facts_family_values::priority_node(b, priority),
{
    reveal(retirement::fact_node);
}

}
