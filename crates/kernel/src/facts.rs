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
    ensures (fact_progress_input(*old(w), i as int, fact)
            ==> progress_measure(*final(w)) < progress_measure(*old(w))
                && dependencies::edges_indexable(*final(w))),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
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
        retirement::selected(fact) ==> final(w).next_question == old(w).next_question,
        retirement::selected(fact) ==> final(w).dependencies@ == old(w).dependencies@,
        retirement::selected(fact) ==> final(w).nodes@[i as int]
            == retirement::fact_node(old(w).nodes@[i as int], fact),
        retirement::selected(fact) && retirement::notices_only(*old(out))
            ==> retirement::notices_only(*final(out)),
        fact is Priority ==> *final(out) == *old(out),
        match fact {
            Fact::PrMerged { head, checks_passed } => old(w).nodes@[i as int].had_pr
                && old(w).nodes@[i as int].merged.is_none()
                && !(old(w).nodes@[i as int].phase is Closed)
                && !(old(w).nodes@[i as int].approved == Some(head) && checks_passed)
                ==> retirement::unapproved_note(*final(out), i),
            _ => true,
        },
        (match fact {
            Fact::Dependency { target, status: DependencyStatus::Delivered } =>
                dependencies::delivery_input(*old(w), i as int, target),
            _ => false,
        } ==> progress::progress_measure(*final(w)) + 1 == progress::progress_measure(*old(w))
            && dependencies::edges_indexable(*final(w))
            && match fact { Fact::Dependency { target, .. } =>
                dependencies::delivery_changed(*old(w), *final(w), i as int, target), _ => false }),
] proof [reveal(dependencies::dependency_frame);] {
    kernel_snapshot! { before_fact = *w }
    kernel_proof! {
        if fact_progress_input(*w, i as int, fact) { fact_witness_premises(*w, i as int, fact); }
    }
    match fact {
        Fact::DependencyRemove { kind, number } => w.remove_dependency(i, kind, number),
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        Fact::PrMerged {
            head,
            checks_passed,
        } => pr_merged(w, i, head, checks_passed, out),
        _ => fact_transition(w, i, fact, out),
    }
    kernel_proof! {
        if fact_progress_input(before_fact@, i as int, fact) {
            if !(fact is Dependency) {
                assert(w.nodes@ =~= before_fact@.nodes@.update(i as int, w.nodes@[i as int]));
                progress::lift_node_progress(before_fact@, *w, i as int);
                dependencies::indexability_frame(before_fact@, *w);
            }
        }
    }
}
}

kernel_fn! {
/// Fact rows / #771: non-dependency, non-merge facts allocate at most one
/// generation, one question and one paid credit, and emit at most two notices.
#[allow(clippy::collapsible_if)]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn fact_transition(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(fact is DependencyRemove || fact is Dependency || fact is PrMerged),
    ensures (fact == Fact::IncidentCleared { by_rob: true }
            && old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && !(old(w).nodes@[i as int].phase is Closed)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
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
        retirement::selected(fact) ==> final(w).next_question == old(w).next_question,
        retirement::selected(fact) ==> final(w).dependencies@ == old(w).dependencies@,
        retirement::selected(fact) ==> final(w).nodes@[i as int]
            == retirement::fact_node(old(w).nodes@[i as int], fact),
        retirement::selected(fact) && retirement::notices_only(*old(out))
            ==> retirement::notices_only(*final(out)),
        fact is Priority ==> *final(out) == *old(out),
        match fact {
            Fact::PrMerged { head, checks_passed } => old(w).nodes@[i as int].had_pr
                && old(w).nodes@[i as int].merged.is_none()
                && !(old(w).nodes@[i as int].phase is Closed)
                && !(old(w).nodes@[i as int].approved == Some(head) && checks_passed)
                ==> retirement::unapproved_note(*final(out), i),
            _ => true,
        },
] proof [reveal(retry_bounds); reveal(retirement::fact_node);] {
    kernel_snapshot! { before_transition = *w }
    kernel_snapshot! { before_out = *out }
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
            }
        }
        Fact::CiFailed { infra } => ci_failed(w, i, infra, out),
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
        Fact::IncidentCleared { by_rob } => incident_cleared(w, i, by_rob, out),
    }
    kernel_proof! {
        if fact is PrDequeuedByPerson || fact is CiFailed || fact is IncidentCleared {
            facts_transition_lemmas::frame_consequences(before_transition@, *w, i, before_out@, *out);
        }
    }
}
}

kernel_fn! {
/// Merge fact rows / Q18 / #771: a late merge earns credit once; other merges
/// enter teardown. Even an unapproved late merge emits at most four notices.
pub(super) fn merge_open(m: &mut Node, head: HeadId) -> (result: ()) [
    requires !(old(m).phase is Closed), node_headroom(*old(m)),
    ensures *final(m) == retirement::stopped(
            Node { merged: Some(head), pr_open: false, ..*old(m) }, Outcome::Merged),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        settle_shape(*old(m)) ==> settle_shape(*final(m)),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd,
        old(m).phase == Phase::Queued && !old(m).paused
            && !old(m).waiting_incident && old(m).effect.is_none()
            ==> distance(*final(m)) < distance(*old(m)),
] {
    kernel_snapshot! { before_merge = *m }
    m.merged = Some(head);
    m.pr_open = false;
    kernel_proof! { retirement::merge_input_distance(before_merge@, head); }
    enter_stopping(m, Outcome::Merged);
}
}

kernel_fn! {
/// State-table merge rows / #805 and #806: compose progress and complete safety.
pub(super) fn pr_merged(w: &mut World, i: usize, head: HeadId, checks_passed: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && old(w).nodes@[i as int].phase == Phase::Queued && !old(w).nodes@[i as int].paused
            && !old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()
            && old(w).nodes@[i as int].question.is_none()
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
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
        final(w).nodes@[i as int] == retirement::fact_node(old(w).nodes@[i as int],
            Fact::PrMerged { head, checks_passed }),
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
        old(w).nodes@[i as int].had_pr && old(w).nodes@[i as int].merged.is_none()
            && !(old(w).nodes@[i as int].phase is Closed)
            && !(old(w).nodes@[i as int].approved == Some(head) && checks_passed)
            ==> retirement::unapproved_note(*final(out), i),
] proof [reveal(retirement::fact_node); reveal(retirement::unapproved_note);] {
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
            merge_open(m, head);
        }
    }
    if !approved {
        notice(out, i, Notice::Incident { needs_rob: false });
        notice(out, i, Notice::Note(NoteKind::UnapprovedMerge));
    }
}
}

kernel_fn! {
/// State-table fact rows / #806: the original match arm, isolated for bounded solver work.
pub(super) fn pr_dequeued(w: &mut World, i: usize, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures retirement::fact_frame(*old(w), *final(w), i, *old(out), *final(out)),
        final(w).nodes@[i as int] == retirement::fact_node(old(w).nodes@[i as int], Fact::PrDequeuedByPerson),
        final(w).next_question == old(w).next_question,
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retirement::fact_frame); reveal(retirement::fact_node); reveal(retry_bounds);] {
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
}

kernel_fn! {
/// State-table fact rows / #806: the original match arm, isolated for bounded solver work.
#[allow(clippy::collapsible_if)]
pub(super) fn ci_failed(w: &mut World, i: usize, infra: bool, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures retirement::fact_frame(*old(w), *final(w), i, *old(out), *final(out)),
        infra ==> final(w).nodes@[i as int] == retirement::fact_node(old(w).nodes@[i as int], Fact::CiFailed { infra }),
        infra ==> final(w).next_question == old(w).next_question,
        infra && retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retirement::fact_frame); reveal(retirement::fact_node); reveal(retry_bounds);] {
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
    ci_infrastructure(w, i, out);
}
}

kernel_fn! {
/// State-table fact rows / #806: the original match arm, isolated for bounded solver work.
pub(super) fn incident_cleared(w: &mut World, i: usize, by_rob: bool, _out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures retirement::fact_frame(*old(w), *final(w), i, *old(_out), *final(_out)),
        final(w).nodes@[i as int] == retirement::fact_node(old(w).nodes@[i as int], Fact::IncidentCleared { by_rob }),
        by_rob && old(w).nodes@[i as int].waiting_incident
            && old(w).nodes@[i as int].effect.is_none()
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && !(old(w).nodes@[i as int].phase is Closed)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int]),
        final(w).next_question == old(w).next_question,
        retirement::notices_only(*old(_out)) ==> retirement::notices_only(*final(_out)),
] proof [reveal(retirement::fact_frame); reveal(retirement::fact_node); reveal(retry_bounds); reveal(distance);] {
    let m = &mut w.nodes[i];
    if m.waiting_incident && (by_rob || !m.incident_needs_rob) {
        m.waiting_incident = false;
        m.incident_needs_rob = false;
    }
}
}

kernel_fn! {
/// State-table publishing/queued CI infrastructure row / Q50 / #806.
/// The existing failure counter and incident update, with no work-credit charge.
pub(super) fn ci_infrastructure(w: &mut World, i: usize, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].phase == Phase::Queued ||
            (old(w).nodes@[i as int].phase == Phase::Publishing
                && current_item(old(w).nodes@[i as int]) == Some(Item::Do(EffectKind::Enqueue))),
    ensures retirement::fact_frame(*old(w), *final(w), i, *old(out), *final(out)),
        final(w).nodes@[i as int] == retirement::fact_node(old(w).nodes@[i as int], Fact::CiFailed { infra: true }),
        final(w).next_question == old(w).next_question,
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] {
    kernel_snapshot! { before_world = *w }
    kernel_snapshot! { before_out = *out }
    kernel_snapshot! { before_ci = w.nodes@[i as int] }
    let escalation = infrastructure_retry(&mut w.nodes[i]);
    if let Some(needs_rob) = escalation {
        raise_incident(w, i, needs_rob, out);
    }
    kernel_proof! {
        retirement::infrastructure_result(before_ci@, w.nodes@[i as int], escalation);
        retirement::infrastructure_frame(before_world@, *w, i, before_out@, *out);
    }
}
}

kernel_fn! {
/// State-table infrastructure failures / Q50 / #806: the existing counter
/// updates and first retry, isolated from the world's frame obligations.
fn infrastructure_retry(m: &mut Node) -> (result: Option<bool>) [
    requires node_headroom(*old(m)), arithmetic_consistent(*old(m)),
    ensures (*final(m), result) == retirement::infrastructure_retry(*old(m)),
        final(m).generation as int <= old(m).generation as int + 1,
] proof [reveal(retirement::infrastructure_retry);] {
    m.run_failures += 1;
    if m.run_failures < 2 {
        set_phase(m, Phase::Publishing); // enqueue again
        None
    } else {
        m.run_failures = 0;
        m.run_incidents = (m.run_incidents + 1).min(2);
        let needs_rob = m.run_incidents >= 2;
        Some(needs_rob)
    }
}
}
