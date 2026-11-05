//! #806: independently gated safety obligations over the unchanged real kernel.
use super::retirement::*;
use super::*;

verus! {
/// State-table fact rows / #806: all per-node consistency and check_nodes rules.
/// One event, no unwind: the real handler has no loop. Counters need only one increment.
pub(super) proof fn fact_preserves_node(w: World, n: Node, fact: Fact)
    requires node_rules(w, n), selected(fact), node_headroom(n),
    ensures node_rules(w, fact_node(n, fact)),
        fact_node(n, fact).proposed == n.proposed,
        n.adopted ==> fact_node(n, fact).adopted,
        fact_node(n, fact).next_seq >= n.next_seq,
        (n.phase is Closed ==> fact_node(n, fact).phase == n.phase
            || (fact_node(n, fact).phase == Phase::Closed(Outcome::Merged)
                && matches!(n.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced)))),
        fact_node(n, fact).credit.earned as int <= n.credit.earned as int + 1,
        (fact_node(n, fact).credit.earned > n.credit.earned
            ==> !n.credit.merge_credited && fact_node(n, fact).credit.merge_credited),
        fact_node(n, fact).credit == n.credit || fact is PrMerged,
        fact_node(n, fact).review_stop == n.review_stop,
        fact_node(n, fact).credit.withheld == n.credit.withheld,
        fact_node(n, fact).attempt == n.attempt, fact_node(n, fact).replanned == n.replanned,
        (fact_node(n, fact).question.is_some() ==> fact_node(n, fact).question == n.question),
        fact_node(n, fact).phase is Planning ==> n.phase is Planning,
        !fact_node(n, fact).holds || n.holds,
        !fact_node(n, fact).pool || n.pool,
        fact_node(n, fact).ticket.is_none() || fact_node(n, fact).ticket == n.ticket,
        fact_node(n, fact).repo == n.repo, fact_node(n, fact).footprint == n.footprint,
{
    reveal(node_rules); reveal(fact_node); reveal(stopped); reveal(incident);
    match fact {
        Fact::IncidentCleared { .. } => {},
        Fact::PrDequeuedByPerson => {},
        Fact::CiFailed { infra: true } => {},
        Fact::PrMerged { .. } => {},
        Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict | Fact::UpstreamStalled => {},
        _ => {},
    }
}
}
