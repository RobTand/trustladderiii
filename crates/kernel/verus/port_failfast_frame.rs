//! Q54 / #440 / #806: compose one local review-stop update with the complete checker.
use super::*;

verus! {
/// Q54: lift the shared completed-operation stop through retirement's exact
/// node/world frames. One actor and arbitrary unchanged second-node state.
pub(super) proof fn completed_stop(before: World, after: World, step: ReviewStop)
    requires retirement::domain(before), retirement::accepted(before),
        node_headroom(before.nodes@[0]),
        !(before.nodes@[0].phase is Stopping || before.nodes@[0].phase is Closed),
        after.nodes@.len() == 2, after.nodes@[1] == before.nodes@[1],
        after.nodes@[0] == retirement::stopped(Node { run: None, job: None, pool: false,
            run_failures: 0, run_incidents: 0, review_stop: Some(step),
            plan_retries: after.nodes@[0].plan_retries,
            effect: match before.nodes@[0].effect {
                Some(e) => Some(InFlight { cancelling: e.cancelling || matches!(e.kind,
                    EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
                None => None,
            }, ..before.nodes@[0] }, Outcome::Stopped),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.pool_capacity == before.pool_capacity,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
    ensures retirement::domain(after), retirement::accepted(after),
        port_failfast_safety::node_post(before.nodes@[0], after.nodes@[0]),
        after.nodes@[0].credit == before.nodes@[0].credit,
        after.nodes@[0].attempt == before.nodes@[0].attempt,
        after.nodes@[0].replanned == before.nodes@[0].replanned,
        after.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        after.nodes@[0].question.is_none(), after.nodes@[0].review_stop == Some(step),
        before.nodes@[0].review_stop.is_none(),
        forall|j: int| 0 <= j < 2 ==>
            after.nodes@[j].proposed == before.nodes@[j].proposed
            && after.nodes@[j].adopted == before.nodes@[j].adopted
            && after.nodes@[j].next_seq == before.nodes@[j].next_seq
            && after.nodes@[j].credit == before.nodes@[j].credit
            && (before.nodes@[j].phase is Closed ==> after.nodes@[j].phase == before.nodes@[j].phase),
{
    reveal(retirement::accepted);
    port_failfast_stop::preserves_completed_node(before, before.nodes@[0], after.nodes@[0], step);
    retirement::node_frame(before, after, after.nodes@[0]);
    retirement::node_frame(before, after, after.nodes@[1]);
    reveal(retirement::stopped);
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}
