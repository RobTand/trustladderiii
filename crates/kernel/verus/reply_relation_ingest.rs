//! Node rules and transition rows over canonical nonretry ingest-family values.
use super::*;

verus! {
pub(super) proof fn protected_rules(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), retirement::node_rules(w, w.nodes@[i]),
        node_headroom(w.nodes@[i]), (match w.nodes@[i].phase { Phase::Ingesting | Phase::Auditing | Phase::Planning { .. } => true, _ => false }),
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::ProtectedPaths))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::ProtectedPaths))),
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::ProtectedPaths))).repo == w.nodes@[i].repo,
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::ProtectedPaths))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::ProtectedPaths))).credit.withheld == w.nodes@[i].credit.withheld,
        (w.nodes@[i].phase is Closed ==> (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::ProtectedPaths))).phase is Closed),
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
    reveal(reply_family_values::ingest);
    reveal(retirement::stopped);
}

pub(super) proof fn agreed_rules(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), retirement::node_rules(w, w.nodes@[i]),
        node_headroom(w.nodes@[i]), w.nodes@[i].phase == Phase::Auditing,
        current_item(w.nodes@[i]) == Some(Item::Await),
    ensures retirement::node_rules(w, reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::AuditAgrees))),
        internal_safety::apply_n2_no_grants(w.nodes@[i], reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::AuditAgrees))),
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::AuditAgrees))).repo == w.nodes@[i].repo,
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::AuditAgrees))).footprint == w.nodes@[i].footprint,
        (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::AuditAgrees))).credit.withheld == w.nodes@[i].credit.withheld,
        (w.nodes@[i].phase is Closed ==> (reply_family_values::finish(w.next_question, reply_family_values::ingest(
            port_t860_contracts::ended(w.nodes@[i]), Reply::AuditAgrees))).phase is Closed),
{
    reveal(retirement::node_rules);
    reveal(port_t860_contracts::ended);
    reveal(reply_family_values::ingest);
    reveal(retirement::stopped);
}

}
