//! #806: small consistency lemmas for the operations used by answer frames.
use super::port_answers_model::*;
use super::*;

verus! {
/// AwaitingRob stop and replacement rows / #806: preserve the complete checker.
#[verifier::spinoff_prover]
pub(super) proof fn stopped_safe(w: World, n: Node, outcome: Outcome)
    requires retirement::node_rules(w, n), n.phase == Phase::AwaitingRob,
        node_headroom(n), outcome == Outcome::Stopped || outcome == Outcome::Replaced,
    ensures retirement::node_rules(w, retirement::stopped(n, outcome)),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
}

/// Stopping MarkHandled row / #806: selected teardown releases only its resource.
#[verifier::spinoff_prover]
pub(super) proof fn handled_safe(w: World, n: Node)
    requires retirement::node_rules(w, n), n.phase is Stopping, n.question.is_some(),
    ensures retirement::node_rules(w, handled(Node { question: None, ..n })),
{ reveal(retirement::node_rules); reveal(handled); }


/// Q18 / #806: arbitrary deposits preserve the full credit ledger under existing headroom.
#[verifier::spinoff_prover]
pub(super) proof fn funded_safe(n: Node, q: Question, deposit: u8)
    requires node_headroom(n), arithmetic_consistent(n), q.cost <= 1,
        n.credit.withheld ==> q.cost == 0,
        !n.credit.ready ==> deposit == 0 && q.cost == 0,
    ensures funded(n.credit, deposit, q.cost).start == n.credit.start,
        funded(n.credit, deposit, q.cost).earned == n.credit.earned,
        funded(n.credit, deposit, q.cost).added as int <= n.credit.added as int + 255,
        funded(n.credit, deposit, q.cost).spent as int == n.credit.spent as int + q.cost,
        funded(n.credit, deposit, q.cost).start as int + funded(n.credit, deposit, q.cost).earned as int
            + funded(n.credit, deposit, q.cost).added as int >= funded(n.credit, deposit, q.cost).spent as int,
        !n.credit.ready ==> funded(n.credit, deposit, q.cost) == n.credit,
{ reveal(funded); }

/// AwaitingRob continuation rows / #806: complete checker independent of the command fence.
#[verifier::spinoff_prover]
pub(super) proof fn continued_safe(w: World, n: Node, q: Question, deposit: u8)
    requires retirement::node_rules(w, n), node_headroom(n),
        n.phase == Phase::AwaitingRob, n.question == Some(q),
        !n.credit.ready ==> deposit == 0 && q.cost == 0,
    ensures retirement::node_rules(w, continued(n, q, deposit)),
{
    reveal(retirement::node_rules);
    funded_safe(n, q, deposit);
    reveal(continued); reveal(rephased); reveal(retirement::stopped);
    reveal(funded);
}

/// CantMeet TryAgain row / #806: resetting retry controls preserves a valid AwaitingRob node.
#[verifier::spinoff_prover]
pub(super) proof fn restarted_safe(w: World, n: Node)
    requires retirement::node_rules(w, n), n.phase == Phase::AwaitingRob,
        n.question.is_some(), n.question.unwrap().kind == QKind::CantMeet,
    ensures retirement::node_rules(w, Node { attempt: 1, review_restarted: false,
        round: 0, replanned: false, ..n }),
{ reveal(retirement::node_rules); }


}
