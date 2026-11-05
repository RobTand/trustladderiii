//! Executable facts transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Fact rows / #771: compose declaration updates, merge credit and the remaining
/// fact handlers. No fact assigns a command ID, effect sequence or ticket.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn on_fact(w: &mut World, i: usize, fact: Fact, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        dependencies::edges_indexable(*old(w)), old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned as int + merge_capacity(final(w).nodes@[i as int])
            <= old(w).nodes@[i as int].credit.earned as int + 1,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(out).len <= old(out).len + 4,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (match fact {
            Fact::Dependency { target, status: DependencyStatus::Delivered } =>
                dependencies::delivery_input(*old(w), i as int, target),
            _ => false,
        } ==> progress::progress_measure(*final(w)) + 1 == progress::progress_measure(*old(w))
            && dependencies::edges_indexable(*final(w))
            && match fact { Fact::Dependency { target, .. } =>
                dependencies::delivery_changed(*old(w), *final(w), i as int, target), _ => false }),
] proof [reveal(dependencies::dependency_frame);] {
    match fact {
        Fact::DependencyRemove { kind, number } => w.remove_dependency(i, kind, number),
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        Fact::PrMerged {
            head,
            checks_passed,
        } => pr_merged(w, i, head, checks_passed, out),
        _ => fact_transition(w, i, fact, out),
    }
}
}

kernel_fn! {
/// Fact rows / #771: non-dependency, non-merge facts allocate at most one
/// generation, one question and one paid credit, and emit at most two notices.
#[allow(clippy::collapsible_if)]
pub(super) fn fact_transition(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(fact is DependencyRemove || fact is Dependency || fact is PrMerged),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    match fact {
        Fact::DependencyRemove { .. } | Fact::Dependency { .. } | Fact::PrMerged { .. } => {}
        Fact::Priority(priority) => {
            if !w.nodes[i].phase.is_closed() {
                w.nodes[i].priority = priority;
            }
        }
        Fact::PrClosedByPerson => {
            let m = &mut w.nodes[i];
            if !m.pr_open {
                return;
            }
            m.pr_open = false;
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                notice(out, i, Notice::Note(NoteKind::PrClosed));
                enter_stopping(m, Outcome::Stopped);
            }
        }
        Fact::PrDequeuedByPerson => {
            let m = &mut w.nodes[i];
            if m.phase != Phase::Queued {
                return;
            }
            // Treated as Rob pausing it: the PR waits until he resumes.
            m.holds = false;
            m.paused = true;
            set_phase(m, Phase::Ready(Resume::Publishing));
            notice(out, i, Notice::Note(NoteKind::PrDequeued));
        }
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
            }
        }
        Fact::CiFailed { infra } => {
            let m = &mut w.nodes[i];
            let before_enqueue =
                m.phase == Phase::Publishing && m.current() == Some(Item::Do(EffectKind::Enqueue));
            if m.phase != Phase::Queued && !before_enqueue {
                return;
            }
            if before_enqueue {
                if let Some(effect) = m.effect.as_mut() {
                    effect.cancelling = true;
                }
            }
            if !infra {
                failed_round(w, i, ReviewStop::CiFailed, out);
                return;
            }
            let m = &mut w.nodes[i];
            m.run_failures += 1;
            if m.run_failures < 2 {
                set_phase(m, Phase::Publishing); // enqueue again
            } else {
                m.run_failures = 0;
                m.run_incidents = (m.run_incidents + 1).min(2);
                let needs_rob = m.run_incidents >= 2;
                raise_incident(w, i, needs_rob, out);
            }
        }
        Fact::Conflict => {
            let m = &mut w.nodes[i];
            if matches!(m.phase, Phase::Publishing | Phase::Queued) {
                set_phase(m, Phase::Rebasing);
            }
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
        Fact::IncidentCleared { by_rob } => {
            let m = &mut w.nodes[i];
            if m.waiting_incident && (by_rob || !m.incident_needs_rob) {
                m.waiting_incident = false;
                m.incident_needs_rob = false;
            }
        }
    }
}
}

kernel_fn! {
/// Merge fact rows / Q18 / #771: a late merge earns credit once; other merges
/// enter teardown. Even an unapproved late merge emits at most four notices.
pub(super) fn pr_merged(w: &mut World, i: usize, head: HeadId, checks_passed: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned as int <= old(w).nodes@[i as int].credit.earned as int + 1,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent == old(w).nodes@[i as int].credit.spent,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 4,
        final(w).nodes@[i as int].credit.earned as int + merge_capacity(final(w).nodes@[i as int])
            <= old(w).nodes@[i as int].credit.earned as int + 1,

        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let m = &mut w.nodes[i];
    if !m.had_pr || m.merged.is_some() {
        return;
    }
    let approved = m.approved == Some(head) && checks_passed;
    match m.phase {
        Phase::Closed(o) => {
            if !matches!(o, Outcome::Stopped | Outcome::Replaced) {
                return;
            }
            // A late merge corrects the record and earns its credit once.
            m.merged = Some(head);
            m.phase = Phase::Closed(Outcome::Merged);
            merge_credit(m);
            notice(out, i, Notice::Note(NoteKind::LateMerge));
            notice(out, i, Notice::Incident { needs_rob: false });
        }
        _ => {
            m.merged = Some(head);
            m.pr_open = false;
            enter_stopping(m, Outcome::Merged);
        }
    }
    if !approved {
        notice(out, i, Notice::Incident { needs_rob: false });
        notice(out, i, Notice::Note(NoteKind::UnapprovedMerge));
    }
}
}
