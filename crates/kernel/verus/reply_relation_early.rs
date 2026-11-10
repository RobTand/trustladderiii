//! Early reply rows over literal completion values.
use super::*;

verus! {
pub(super) proof fn ingested_rules(w: World, i: int, reply: Reply, delivered: bool)
    requires 0 <= i < w.nodes@.len(), retirement::node_rules(w, w.nodes@[i]),
        node_headroom(w.nodes@[i]), w.nodes@[i].phase == Phase::Ingesting,
        (match reply { Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) => true, _ => false }),
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_ingest_world::final_node(port_t860_contracts::ended(w.nodes@[i]), reply, delivered)),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_ingest_world::final_node(port_t860_contracts::ended(w.nodes@[i]), reply, delivered)),
        (reply_family_ingest_world::final_node(port_t860_contracts::ended(w.nodes@[i]), reply, delivered)).repo == w.nodes@[i].repo,
        (reply_family_ingest_world::final_node(port_t860_contracts::ended(w.nodes@[i]), reply, delivered)).footprint == w.nodes@[i].footprint,
        (reply_family_ingest_world::final_node(port_t860_contracts::ended(w.nodes@[i]), reply, delivered)).credit.withheld ==> w.nodes@[i].credit.withheld || !delivered,
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
    reveal(reply_family_values::ingest);
    reveal(retirement::stopped);
    reveal(reply_family_ingest_world::final_node);
    reveal(reply_family_ingest_world::withheld);
}

pub(super) proof fn disagreed_rules(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), retirement::node_rules(w, w.nodes@[i]),
        node_headroom(w.nodes@[i]), w.nodes@[i].phase == Phase::Auditing,
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question, reply_family_values::ingest(port_t860_contracts::ended(w.nodes@[i]), Reply::AuditDisagrees))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question, reply_family_values::ingest(port_t860_contracts::ended(w.nodes@[i]), Reply::AuditDisagrees))),
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(port_t860_contracts::ended(w.nodes@[i]), Reply::AuditDisagrees))).repo == w.nodes@[i].repo,
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(port_t860_contracts::ended(w.nodes@[i]), Reply::AuditDisagrees))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(port_t860_contracts::ended(w.nodes@[i]), Reply::AuditDisagrees))).credit.withheld == w.nodes@[i].credit.withheld,
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
    reveal(reply_family_values::ingest);
    reveal(retirement::stopped);
    reveal(port_t860_contracts::audit_node);
}

pub(super) proof fn plan_check_rules(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), retirement::node_rules(w, w.nodes@[i]),
        node_headroom(w.nodes@[i]), w.nodes@[i].phase is Planning,
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question, reply_family_values::plan_check(port_t860_contracts::ended(w.nodes@[i])))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question, reply_family_values::plan_check(port_t860_contracts::ended(w.nodes@[i])))),
        (reply_family_values::finish(w.next_question, reply_family_values::plan_check(port_t860_contracts::ended(w.nodes@[i])))).repo == w.nodes@[i].repo,
        (reply_family_values::finish(w.next_question, reply_family_values::plan_check(port_t860_contracts::ended(w.nodes@[i])))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question, reply_family_values::plan_check(port_t860_contracts::ended(w.nodes@[i])))).credit.withheld == w.nodes@[i].credit.withheld,
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
    reveal(reply_family_values::ingest);
    reveal(retirement::stopped);
    reveal(reply_family_values::plan_check);
}

pub(super) proof fn ingest_safety(w: World, i: usize, reply: Reply, delivered: bool,
    ob: Outbox, oa: Outbox)
    requires i < w.nodes@.len(), retirement::node_rules(w, w.nodes@[i as int]),
        node_headroom(w.nodes@[i as int]), current_item(w.nodes@[i as int]) == Some(Item::Await),
        port_t860_contracts::admissible(w, w.nodes@[i as int], reply),
        (match reply { Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees
            | Reply::AuditDisagrees | Reply::ProtectedPaths => true, _ => false }),
        ob.len < OUTBOX,
        reply_family_values::resolved(reply_family_values::ingest(port_t860_contracts::ended(w.nodes@[i as int]), reply), i as int, ob, oa),
    ensures transition_safety::node_safety(w.nodes@[i as int],
        reply_family_ingest_world::final_node(port_t860_contracts::ended(w.nodes@[i as int]), reply, delivered),
        w.fail_fast, Event::Reply { node: NodeId(i as u64), generation: w.nodes@[i as int].generation, reply }, i as int, ob, oa),
{
    reveal(transition_safety::node_safety);
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::admissible);
    reveal(port_t860_contracts::ended);
    reveal(reply_family_values::ingest);
    reveal(reply_family_ingest_world::final_node);
    reveal(reply_family_ingest_world::withheld);
    reveal(port_t860_contracts::audit_node);
    reveal(retirement::stopped);
}
}
