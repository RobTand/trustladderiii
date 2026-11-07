//! Exact pre-extraction run_failed reference, including erased proof annotations.
use crate::*;

kernel_fn! {
/// Failed-run rows / #771: a retry and incident allocate at most two generations.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn run_failed(w: &mut World, i: usize, out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].run_failures <= 1,
        old(w).nodes@[i as int].run_incidents <= 2,
    ensures report::failed_round(old(report).entries@, final(report).entries@,
            NodeId(i as u64), FailedRoundCause::RunFailed),
        port_t860_contracts::failed_post(old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].run_failures <= 1,
        final(w).nodes@[i as int].run_incidents <= 2,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    report.record_failed_round(NodeId(i as u64), FailedRoundCause::RunFailed);
    kernel_snapshot! { before_failed = w.nodes@[i as int] }
    kernel_snapshot! { before_failed_out = *out }
    let m = &mut w.nodes[i];
    abandon_operation(m);
    m.pos = operation_start(m.phase);
    m.generation += 1;
    m.run_failures += 1;
    if m.run_failures >= 2 {
        // Every second failure raises an incident. Only Rob can clear the second one.
        m.run_failures = 0;
        m.run_incidents = (m.run_incidents + 1).min(2);
        let needs_rob = m.run_incidents >= 2;
        kernel_proof! {
            assert(retirement::incident(*m, needs_rob) == retries_facts::failed_run(before_failed@)) by {
                retries_facts::incident(before_failed@, *m, needs_rob);
            }
        }
        raise_incident(w, i, needs_rob, out);
    }
    kernel_proof! {
        if before_failed@.run_failures == 0 {
            assert(w.nodes@[i as int] == retries_facts::failed_run(before_failed@)) by {
                retries_facts::restarted(before_failed@, w.nodes@[i as int]);
            }
        }
        assert(port_t860_contracts::failed_post(before_failed@, w.nodes@[i as int], before_failed_out@, *out)) by {
            retries_facts::failed_result(before_failed@, w.nodes@[i as int], before_failed_out@, *out);
        }
    }
}
}
