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

pub(super) proof fn on_fact_rows(before: World, after: World, i: int,
    fact: Fact, ob: Outbox, oa: Outbox)
    requires 0 <= i < before.nodes@.len(),
        ob.len + 4 <= OUTBOX,
        oa.len <= ob.len + 4,
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        false ==> after.nodes@[i]
            == retirement::fact_node(before.nodes@[i], fact),
        retirement::selected(fact) && retirement::notices_only(ob)
            ==> retirement::notices_only(oa),
    ensures retirement::selected(fact) && empty_outbox(ob)
        ==> transition_safety::nodes_step(before, after)
            && transition_safety::outputs_ok(before, after,
                transition_safety::emitted(ob, oa), None),
{
    if retirement::selected(fact) && empty_outbox(ob) {
        assert(ob.len == 0) by { reveal(empty_outbox); }
        assert(retirement::notices_only(ob)) by {
            reveal(empty_outbox);
            reveal(retirement::notices_only);
        }
        assert(after.nodes@ =~= before.nodes@.update(i,
            retirement::fact_node(before.nodes@[i], fact)));
        selected_apply_rows(before, after, i, fact, ob, oa);
    }
}
}
