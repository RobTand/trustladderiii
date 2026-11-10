use super::*;

verus! {

pub(super) proof fn priority(before: World, after: World, i: int, priority: Priority,
    ob: Outbox, oa: Outbox)
    requires checking::accepted(before), 0 <= i < before.nodes@.len(), i <= u64::MAX,
        retirement::node_rules(before, after.nodes@[i]),
        internal_safety::apply_n2_no_grants(before.nodes@[i], after.nodes@[i]),
        dependencies::same_endpoint(before.nodes@[i], after.nodes@[i]),
        transition_safety::node_safety(before.nodes@[i], after.nodes@[i], before.fail_fast,
            Event::Fact { node: NodeId(i as u64), fact: Fact::Priority(priority) }, i, ob, oa),
        after.nodes@[i].repo == before.nodes@[i].repo,
        after.nodes@[i].footprint == before.nodes@[i].footprint,
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        ob.len <= OUTBOX, oa == ob,
    ensures checking::accepted(after),
        transition_safety::transition_safety(before, after,
            transition_safety::Transition::Apply(Event::Fact {
                node: NodeId(i as u64), fact: Fact::Priority(priority) }),
            transition_safety::emitted(ob, oa)),
{
    vstd::std_specs::vec::axiom_spec_len(&before.nodes);
    retirement::node_frame(before, after, after.nodes@[i]);
    checking::accepted_parts(before);
    if !dependencies::prerequisites_delivered(before, NodeId(i as u64)) {
        dependency_owner_exports::waiting_owner_ok(before, i);
    }
    assert(after.nodes@ =~= before.nodes@.update(i, after.nodes@[i]));
    internal_safety::apply_step_preserves_accepted_full(before, after, i);
    apply_safety::apply_frame(before, after);
    transition_safety::lift_apply(before, after,
        Event::Fact { node: NodeId(i as u64), fact: Fact::Priority(priority) }, i, ob, oa);
}

}
