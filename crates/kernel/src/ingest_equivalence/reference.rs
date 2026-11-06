//! Verbatim unextracted #832-A reply_ingest body from main 6a1f337adc, test-only.
use super::*;

pub(super) fn reference(w: &mut World, i: usize, reply: Reply, out: &mut Outbox) {
    match reply {
        Reply::IngestDone { size } => {
            finish_ingest(w, i, size, None);
        }
        Reply::IngestEarlyEnd(e) => {
            finish_ingest(w, i, 1, Some(e));
        }
        Reply::AuditAgrees => match w.nodes[i].early_end {
            Some(e) => enter_stopping(&mut w.nodes[i], e.outcome()),
            None => {
                notice(out, i, Notice::MeasuresPosted);
                set_phase(&mut w.nodes[i], Phase::Unplanned);
            }
        },
        Reply::AuditDisagrees => {
            let m = &mut w.nodes[i];
            m.audit_rounds = m.audit_rounds.saturating_add(1);
            if m.audit_rounds < 2 {
                // Ingest revises once with the audit's objections.
                set_phase(m, Phase::Ingesting);
            } else {
                // Q27, #832-A: intake cannot split. The one repair is spent.
                stop_for_review(m, ReviewStop::AuditDisagreement);
            }
        }
        Reply::ProtectedPaths => enter_stopping(&mut w.nodes[i], Outcome::NotActionable),
        _ => {}
    }
}
