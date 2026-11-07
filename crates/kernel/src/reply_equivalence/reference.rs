//! #860: verbatim pre-extraction on_reply body at 6a1f337adc6df8b1b8c4de135f4c96973389457e.
use crate::*;

pub(super) fn on_reply(w: &mut World, i: usize, generation: u64, reply: Reply, out: &mut Outbox) {
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_outputs = *out }
    let n = &w.nodes[i];
    if n.phase.is_stopping() {
        // The run or job ended on its own, so teardown needn't stop it.
        let m = &mut w.nodes[i];
        if m.run == Some(generation) {
            m.run = None;
            m.pool = false;
        }
        if m.job == Some(generation) {
            m.job = None;
        }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        return;
    }
    if n.current() != Some(Item::Await) || (n.run != Some(generation) && n.job != Some(generation))
    {
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        return; // stale
    }
    // A reply that breaks its schema or names the wrong commit counts as a failed run.
    kernel_proof! {
        if successful_reply_input(*n, reply) {
            if let Reply::PlanLeaf { footprint, .. } = reply {
                if footprint::footprint_empty(n.footprint) { footprint::inline_is_valid(*w, 1); }
            }
        }
    }
    if !accepts(w, n, &reply) {
        run_failed(w, i, out, &mut Report::new());
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        return;
    }
    kernel_snapshot! { before_operation = *w }
    end_operation(&mut w.nodes[i]);
    kernel_proof! {
        footprint::valid_under_table_growth(before_operation@, *w, w.nodes@[i as int].footprint);
    }
    kernel_snapshot! { after_operation = *w }
    kernel_proof! {
        progress_gate_contracts::table_order_frame(before_reply@, *w);
        if successful_reply_input(before_reply@.nodes@[i as int], reply) {
            progress_gate_contracts::end_reply_progress(before_reply@.nodes@[i as int],
                w.nodes@[i as int], reply);
        }
    }
    match reply {
        Reply::MakerNeedsFiles { files, fits } => {
            kernel_proof! { footprint::valid_under_table_growth(before_operation@, *w, files); }
            reply_files(w, i, files, fits, out);
            kernel_proof! {
                progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                    generation, reply, before_outputs@, *out);
            }
        }
        _ => {
            reply_transition(w, i, reply, out, &mut Report::new());
            kernel_proof! {
                progress_gate_contracts::reply_bounds_result(after_operation@, *w, i as int,
                    before_outputs@.len, out.len);
                progress_gate_contracts::table_order_frame(after_operation@, *w);
                progress_gate_contracts::maker_reply_composed(before_reply@, after_operation@,
                    *w, i as int, generation, reply, before_outputs@, *out);
                if let Reply::PlanLeaf { footprint, .. } = reply {
                    footprint::valid_under_table_growth(before_operation@, *w, footprint);
                }
                footprint::valid_under_table_growth(before_operation@, *w, before_operation@.nodes@[i as int].footprint);
            }
        }
    }
}


// Exact pre-K2-rest failed-run body, including proof macros erased in tests.
pub(super) fn run_failed_original(w: &mut World, i: usize, out: &mut Outbox, report: &mut Report) {
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
