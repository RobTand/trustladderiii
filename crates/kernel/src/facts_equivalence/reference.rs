//! Verbatim pre-extraction bodies at a7e6226246324c3b1bcf4003bad4f64051265d21.
use crate::*;

#[allow(clippy::collapsible_if)]
pub(super) fn fact_transition(w: &mut World, i: usize, fact: Fact, out: &mut Outbox, report: &mut Report) {
    kernel_snapshot! { before_report = report.entries@ }
    kernel_snapshot! { report_phase = w.nodes@[i as int].phase }
    kernel_snapshot! { report_item = current_item(w.nodes@[i as int]) }
    kernel_proof! { report::lemmas::fact_kind(report_phase@, report_item@, fact); }
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
                kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                    NodeId(i as u64), report::fact_failure(report_phase@, report_item@, fact)); }
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
                kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                    NodeId(i as u64), report::fact_failure(report_phase@, report_item@, fact)); }
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
        Fact::CiFailed { infra } => {
            ci_failed(w, i, infra, out, report);
            kernel_proof! { report::lemmas::round_delta(before_report@, report.entries@,
                NodeId(i as u64), report::ci_failure(report_phase@, report_item@, infra)); }
        }
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
                kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                    NodeId(i as u64), report::fact_failure(report_phase@, report_item@, fact)); }
                return;
            }
            m.run_incidents = (m.run_incidents + 1).min(2);
            let needs_rob = m.run_incidents >= 2;
            raise_incident(w, i, needs_rob, out);
        }
        Fact::IncidentCleared { by_rob } => incident_cleared(w, i, by_rob, out),
    }
    kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                NodeId(i as u64), report::fact_failure(report_phase@, report_item@, fact)); }
    kernel_proof! {
        if fact is PrDequeuedByPerson || fact is CiFailed || fact is IncidentCleared {
            facts_transition_lemmas::frame_consequences(before_transition@, *w, i, before_out@, *out);
        }
    }
}

#[allow(clippy::collapsible_if)]
pub(super) fn ci_failed(w: &mut World, i: usize, infra: bool, out: &mut Outbox, report: &mut Report) {
    kernel_snapshot! { before_report = report.entries@ }
    kernel_snapshot! { report_phase = w.nodes@[i as int].phase }
    kernel_snapshot! { report_item = current_item(w.nodes@[i as int]) }
    kernel_proof! { report::lemmas::ci_kind(report_phase@, report_item@, infra); }
    kernel_snapshot! { before_ci = w.nodes@[i as int] }
    let m = &mut w.nodes[i];
    let before_enqueue =
        m.phase == Phase::Publishing && m.current() == Some(Item::Do(EffectKind::Enqueue));
    if m.phase != Phase::Queued && !before_enqueue {
        kernel_proof! { reveal(retirement::fact_node); }
        kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                NodeId(i as u64), report::ci_failure(report_phase@, report_item@, infra)); }
        return;
    }
    if before_enqueue {
        if let Some(effect) = m.effect.as_mut() {
            effect.cancelling = true;
        }
    }
    kernel_snapshot! { prepared_ci = *w }
    kernel_snapshot! { prepared_out = *out }
    if !infra {
        failed_round(w, i, ReviewStop::CiFailed, out, report);
        kernel_proof! {
            report::lemmas::failure_kind(ReviewStop::CiFailed);
            report::lemmas::read_failed_round(before_report@, report.entries@,
                NodeId(i as u64), FailedRoundCause::CiFailed);
        }
        kernel_proof! {
            facts_ci_failed_lemmas::retry_frame(prepared_ci@, *w, i, prepared_out@, *out);
        }
        kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                NodeId(i as u64), report::ci_failure(report_phase@, report_item@, infra)); }
        return;
    }
    kernel_proof! {
        facts_ci_failed_lemmas::cancellation_input(before_ci@, w.nodes@[i as int]);
    }
    ci_infrastructure(w, i, out);
    kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
                NodeId(i as u64), report::ci_failure(report_phase@, report_item@, infra)); }
}
