//! K5 Fact projections for node safety and notice-only outputs.
use super::*;

verus! {
pub(super) proof fn notices_outputs(before: World, after: World, ob: Outbox, oa: Outbox)
    requires ob.len <= oa.len <= OUTBOX, retirement::notices_only(oa),
    ensures transition_safety::no_effects(transition_safety::emitted(ob, oa)),
        transition_safety::outputs_ok(before, after, transition_safety::emitted(ob, oa), None),
{
    reveal(retirement::notices_only);
    reveal(transition_safety::no_effects);
    assert forall|k: int| 0 <= k < transition_safety::emitted(ob, oa).len()
        implies match #[trigger] transition_safety::emitted(ob, oa)[k] {
            Some(Out::Effect { .. }) => false,
            _ => true,
        } by {
        assert(0 <= ob.len as int + k < OUTBOX);
        assert(transition_safety::emitted(ob, oa)[k] == oa.items@[ob.len as int + k]);
        assert(match oa.items@[ob.len as int + k] {
            None | Some(Out::Notice { .. }) => true,
            _ => false,
        });
    }
    transition_safety::outputs_quiet(before, after, transition_safety::emitted(ob, oa), None);
}

pub(super) proof fn selected_apply_rows(before: World, after: World, i: int,
    fact: Fact, ob: Outbox, oa: Outbox)
    requires 0 <= i < before.nodes@.len(), retirement::selected(fact),
        after.nodes@ == before.nodes@.update(i, retirement::fact_node(before.nodes@[i], fact)),
        ob.len <= oa.len <= OUTBOX, retirement::notices_only(oa),
    ensures transition_safety::nodes_step(before, after),
        transition_safety::outputs_ok(before, after, transition_safety::emitted(ob, oa), None),
{
    apply_safety::selected_fact_step(before.nodes@[i], fact);
    assert forall|j: int| 0 <= j < before.nodes@.len()
        implies transition_safety::node_step(before.nodes@[j], #[trigger] after.nodes@[j]) by {
        if j == i {
            assert(after.nodes@[j] == retirement::fact_node(before.nodes@[i], fact));
        } else {
            assert(after.nodes@[j] == before.nodes@[j]);
        }
    }
    notices_outputs(before, after, ob, oa);
}
}
