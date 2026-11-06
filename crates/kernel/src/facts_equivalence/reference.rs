//! #806: verbatim pre-extraction fact_transition body at c4dc744250292bbee67b6175951ef391beeba965.
use crate::*;

#[allow(clippy::collapsible_if)]
pub(super) fn fact_transition(w: &mut World, i: usize, fact: Fact, out: &mut Outbox) {
    kernel_snapshot! { before_transition = *w }
    kernel_snapshot! { before_out = *out }
    match fact {
        Fact::DependencyRemove { .. } | Fact::Dependency { .. } | Fact::PrMerged { .. } => {}
        Fact::Priority(priority) => {
            if !w.nodes[i].phase.is_closed() {
                w.nodes[i].priority = priority;
            }
            kernel_proof! { reveal(retirement::fact_node); }
        }
        Fact::PrClosedByPerson => {
            let m = &mut w.nodes[i];
            if !m.pr_open {
                kernel_proof! { reveal(retirement::fact_node); }
                return;
            }
            m.pr_open = false;
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                notice(out, i, Notice::Note(NoteKind::PrClosed));
                enter_stopping(m, Outcome::Stopped);
            }
            kernel_proof! { reveal(retirement::fact_node); }
        }
        Fact::PrDequeuedByPerson => pr_dequeued(w, i, out),
        Fact::PushByOther { head } => {
            let m = &w.nodes[i];
            if !m.had_pr {
                return;
            }
            notice(out, i, Notice::Note(NoteKind::OutsidePush));
            let published = matches!(
                m.phase,
                Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing)
            );
            if published {
                update_recheck_head(&mut w.nodes[i], head);
                recheck(w, i, Phase::Ready(Resume::Checking), out);
                kernel_proof! { reveal(retry_bounds); }
            }
        }
        Fact::CiFailed { infra } => ci_failed(w, i, infra, out),
        Fact::Conflict => {
            let m = &mut w.nodes[i];
            if matches!(m.phase, Phase::Publishing | Phase::Queued) {
                set_phase(m, Phase::Rebasing);
            }
            kernel_proof! { reveal(retirement::fact_node); }
        }
        Fact::UpstreamStalled => {
            let m = &mut w.nodes[i];
            if m.phase != Phase::Queued || m.waiting_incident {
                return;
            }
            m.run_incidents = (m.run_incidents + 1).min(2);
            let needs_rob = m.run_incidents >= 2;
            raise_incident(w, i, needs_rob, out);
        }
        Fact::IncidentCleared { by_rob } => incident_cleared(w, i, by_rob, out),
    }
    kernel_proof! {
        if fact is PrDequeuedByPerson || fact is CiFailed || fact is IncidentCleared {
            facts_transition_lemmas::frame_consequences(before_transition@, *w, i, before_out@, *out);
        }
    }
}
