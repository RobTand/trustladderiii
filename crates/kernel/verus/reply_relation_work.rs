//! Full node-rule preservation over canonical successful reply values.
use super::*;

verus! {
pub(super) proof fn made_rules(w: World, i: int, head: HeadId)
    requires 0 <= i < w.nodes@.len(),
        retirement::node_rules(w, w.nodes@[i]), node_headroom(w.nodes@[i]),
        w.nodes@[i].phase == Phase::Making,
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question,
            reply_family_values::work_made(port_t860_contracts::ended(w.nodes@[i]), head))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question,
            reply_family_values::work_made(port_t860_contracts::ended(w.nodes@[i]), head))),
        (reply_family_values::finish(w.next_question,
            reply_family_values::work_made(port_t860_contracts::ended(w.nodes@[i]), head))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question,
            reply_family_values::work_made(port_t860_contracts::ended(w.nodes@[i]), head))).credit.withheld == w.nodes@[i].credit.withheld,
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
}

pub(super) proof fn checked_rules(w: World, i: int)
    requires 0 <= i < w.nodes@.len(),
        retirement::node_rules(w, w.nodes@[i]), node_headroom(w.nodes@[i]),
        w.nodes@[i].phase == Phase::Checking,
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question,
            reply_family_values::work_checked(port_t860_contracts::ended(w.nodes@[i])))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question,
            reply_family_values::work_checked(port_t860_contracts::ended(w.nodes@[i])))),
        (reply_family_values::finish(w.next_question,
            reply_family_values::work_checked(port_t860_contracts::ended(w.nodes@[i])))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question,
            reply_family_values::work_checked(port_t860_contracts::ended(w.nodes@[i])))).credit.withheld == w.nodes@[i].credit.withheld,
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
}

pub(super) proof fn approved_rules(w: World, i: int, head: HeadId)
    requires 0 <= i < w.nodes@.len(),
        retirement::node_rules(w, w.nodes@[i]), node_headroom(w.nodes@[i]),
        w.nodes@[i].phase == Phase::Reviewing,
        current_item(w.nodes@[i]) == Some(Item::Await),
        w.nodes@[i].head == Some(head),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question,
            reply_family_values::work_approved(port_t860_contracts::ended(w.nodes@[i]), head))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question,
            reply_family_values::work_approved(port_t860_contracts::ended(w.nodes@[i]), head))),
        (reply_family_values::finish(w.next_question,
            reply_family_values::work_approved(port_t860_contracts::ended(w.nodes@[i]), head))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question,
            reply_family_values::work_approved(port_t860_contracts::ended(w.nodes@[i]), head))).credit.withheld == w.nodes@[i].credit.withheld,
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
}

}
