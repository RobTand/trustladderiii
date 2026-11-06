//! Pre-extraction ingest dispatcher for conformance.
use super::*;

pub(super) fn reference(w: &mut World, i: usize, reply: Reply, out: &mut Outbox) {
    match reply {
        Reply::IngestDone { size } => {
            finish_ingest(w, i, size, None);
        }
        Reply::IngestEarlyEnd(e) => {
            finish_ingest(w, i, 1, Some(e));
        }
        Reply::AuditAgrees => {
            kernel_proof! { reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); }
            match w.nodes[i].early_end {
                Some(e) => enter_stopping(&mut w.nodes[i], e.outcome()),
                None => {
                    notice(out, i, Notice::MeasuresPosted);
                    set_phase(&mut w.nodes[i], Phase::Unplanned);
                }
            }
        }
        Reply::AuditDisagrees => {
            kernel_snapshot! { before_audit = w.nodes@[i as int] }
            let m = &mut w.nodes[i];
            m.audit_rounds = m.audit_rounds.saturating_add(1);
            if m.audit_rounds < 2 {
                // Ingest revises once with the audit's objections.
                set_phase(m, Phase::Ingesting);
            } else {
                // Intake cannot split. The one repair is spent.
                stop_for_review(m, ReviewStop::AuditDisagreement);
            }
            kernel_proof! { port_t860_link::audit_finished(before_audit@, w.nodes@[i as int]); }
        }
        Reply::ProtectedPaths => {
            kernel_proof! { reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); }
            enter_stopping(&mut w.nodes[i], Outcome::NotActionable);
        }
        _ => {}
    }
}
