use super::*;

verus! {

pub(super) proof fn node_rows(w: World, b: Node, priority: Priority, i: usize, out: Outbox)
    requires retirement::node_rules(w, b), node_headroom(b), i <= u64::MAX,
    ensures retirement::node_rules(w, facts_family_values::priority_node(b, priority)),
        dependencies::same_endpoint(b, facts_family_values::priority_node(b, priority)),
        transition_safety::node_safety(b, facts_family_values::priority_node(b, priority),
            w.fail_fast, Event::Fact { node: NodeId(i as u64), fact: Fact::Priority(priority) },
            i as int, out, out),
{
    let fact = Fact::Priority(priority);
    retirement::fact_preserves_node(w, b, fact);
    fact_relation_reveal::priority_model(b, priority);
    facts_family_values::value_node_step(b, facts_family_values::priority_node(b, priority),
        w.fail_fast, i, fact);
    facts_family_values::value_fail_fast(b, facts_family_values::priority_node(b, priority),
        w.fail_fast, i, fact);
    reveal(transition_safety::node_safety);
}

}
