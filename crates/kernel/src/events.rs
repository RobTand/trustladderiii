//! Executable events transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// #771: compose the actual event handlers' per-apply counter bounds.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
#[allow(clippy::collapsible_if)]
pub(super) fn apply(w: &mut World, event: Event, out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures report::agrees_except_fact_stops_and_ci_failed(event,
            phase_view(old(w).nodes@), phase_view(final(w).nodes@), report_input(*old(w), event),
            old(report).entries@, final(report).entries@),
(progress_input(*old(w), event)
            ==> progress_measure(*final(w)) < progress_measure(*old(w))
                && dependencies::edges_indexable(*final(w))),
        forall|j: int| 0 <= j < final(w).nodes@.len() ==>
            settle_shape(final(w).nodes@[j])
                && footprint::valid_footprint(*final(w), final(w).nodes@[j].footprint),
        forall|j: int| 0 <= j < old(w).nodes@.len() ==>
            (old(w).nodes@[j].phase is Closed ==> final(w).nodes@[j].phase is Closed)
                && final(w).nodes@[j].repo == old(w).nodes@[j].repo
                && final(w).nodes@[j].last_cmd >= old(w).nodes@[j].last_cmd,
        old(w).nodes@.len() <= final(w).nodes@.len() <= old(w).nodes@.len() + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@, final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        forall|j: int| 0 <= j < old(w).nodes@.len() ==>
            final(w).nodes@[j].generation as int <= old(w).nodes@[j].generation as int + 2
            && final(w).nodes@[j].next_seq as int <= old(w).nodes@[j].next_seq as int + 1
            && final(w).nodes@[j].credit.start as int <= old(w).nodes@[j].credit.start as int + CREDIT_START_STEP_BUDGET
            && final(w).nodes@[j].credit.earned as int + merge_capacity(final(w).nodes@[j])
                <= old(w).nodes@[j].credit.earned as int + 1
            && final(w).nodes@[j].credit.added as int <= old(w).nodes@[j].credit.added as int + CREDIT_ADDED_STEP_BUDGET
            && final(w).nodes@[j].credit.spent as int <= old(w).nodes@[j].credit.spent as int + CREDIT_SPENT_STEP_BUDGET,
        forall|j: int| old(w).nodes@.len() <= j < final(w).nodes@.len() ==>
            final(w).nodes@[j].last_cmd == 0
            && final(w).nodes@[j].generation == 0 && final(w).nodes@[j].next_seq == 0
            && final(w).nodes@[j].credit.start == 0 && final(w).nodes@[j].credit.earned == 0
            && final(w).nodes@[j].credit.added == 0 && final(w).nodes@[j].credit.spent == 0,
        (old(w).nodes@.len() > 0 && old(w).nodes@[0].phase == Phase::Unplanned
            && progress_unplanned::obstructed(old(w).nodes@[0])
            && event == progress_unplanned::outside_event(old(w).nodes@[0]))
            ==> final(w).nodes@.len() == old(w).nodes@.len()
                && (forall|j: int| 1 <= j < old(w).nodes@.len()
                    ==> final(w).nodes@[j] == old(w).nodes@[j]),
        match event {
            Event::Fact { node, fact } => retirement::selected(fact)
                && node.0 < old(w).nodes@.len() ==> {
                    final(w).nodes@ == old(w).nodes@.update(node.0 as int,
                        retirement::fact_node(old(w).nodes@[node.0 as int], fact))
                    && final(w).next_question == old(w).next_question
                    && final(w).file_sets@ == old(w).file_sets@
                    && final(w).dependencies@ == old(w).dependencies@
                    && (retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)))
                    && match fact {
                        Fact::Priority(_) => *final(out) == *old(out),
                        Fact::PrMerged { head, checks_passed } => old(w).nodes@[node.0 as int].had_pr
                            && old(w).nodes@[node.0 as int].merged.is_none()
                            && !(old(w).nodes@[node.0 as int].phase is Closed)
                            && !(old(w).nodes@[node.0 as int].approved == Some(head) && checks_passed)
                            ==> retirement::unapproved_note(*final(out), node.0 as usize),
                        _ => true,
                    }
                },
            _ => true,
        },
        progress_gate_apply::resume_apply_post(*old(w), *final(w), event),
        (match event {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table); reveal(progress_input); reveal(fresh_commands);] {
    kernel_snapshot! { report_before = report.entries@ }
    kernel_proof! { report_projection(*w, event); report::lemmas::entries_for_event(event, report_input(*w, event)); }
    let target = match event {
        Event::Reply { node, .. } | Event::Ack { node, .. } | Event::Fact { node, .. }
            | Event::Command { node, .. } => Some(node),
        _ => None,
    };
    let before_phase = match target {
        Some(node) => match w.index(node) {
            Some(i) => Some(w.nodes[i].phase),
            None => None,
        },
        None => None,
    };
    kernel_snapshot! { before_apply = *w }
    kernel_snapshot! { before_apply_out = *out }
    kernel_proof! { events_lemmas::selected_input(before_apply@, event); }
    match event {
        Event::NewAdoption {
            repo,
            head,
            priority,
        } => {
            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
                let mut node = Node::new(repo);
                node.proposed = Some(head);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::NewRequest { repo, priority } => {
            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
                let mut node = Node::new(repo);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::Reply {
            node,
            generation,
            reply,
        } => {
            if let Some(i) = w.index(node) {
                on_reply(w, i, generation, reply, out, report);
                kernel_proof! {
                    if progress_input(before_apply@, event) {
                        assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                        progress::lift_node_progress(before_apply@, *w, i as int);
                        dependencies::indexability_frame(before_apply@, *w);
                    }
                }
            }
        }
        Event::Ack { node, seq, ack } => {
            if let Some(i) = w.index(node) {
                on_ack(w, i, seq, ack, out);
                kernel_proof! {
                    if progress_input(before_apply@, event) {
                        assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                        progress::lift_node_progress(before_apply@, *w, i as int);
                        dependencies::indexability_frame(before_apply@, *w);
                    }
                }
            }
        }
        Event::Fact { node, fact } => {
            if let Some(i) = w.index(node) {
                on_fact(w, i, fact, out, report);
            }
        }
        Event::Command { node, id, command } => {
            if let Some(i) = w.index(node) {
                on_command(w, i, id, command, out);
                kernel_proof! {
                    if command == Command::Resume {
                        progress_gate_apply_resume_frame::compose(before_apply@, *w, node, id);
                    }
                    if progress_input(before_apply@, event) {
                        assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                        progress::lift_node_progress(before_apply@, *w, i as int);
                        dependencies::indexability_frame(before_apply@, *w);
                    }
                }
            }
        }
    }
    kernel_snapshot! { report_middle = report.entries@ }
    if let Some(node) = if matches!(event, Event::Fact { .. }) { None } else { target } {
        if let Some(before) = before_phase {
            if let Some(i) = w.index(node) {
                let after = w.nodes[i].phase;
                let cause = StopCause::from_phase(after, w.nodes[i].review_stop,
                    false,
                    matches!(event, Event::Command { command: Command::Stop, .. }
                        | Event::Command { command: Command::Answer { answer: Answer::Stop, .. }, .. }));
                report.record_stop(node, before, after, cause);
            }
        }
    }
    kernel_snapshot! { stop_review = match report::event_target(event) {
        Some(node) if node.0 < w.nodes@.len() => w.nodes@[node.0 as int].review_stop,
        _ => None,
    } }
    kernel_proof! {
        {
            assert forall|j: int| 0 <= j < before_apply@.nodes@.len()
                && report::event_target(event) != Some(NodeId(j as u64))
                implies report::terminal_phase_frame(before_apply@.nodes@[j].phase, w.nodes@[j].phase) by {
                report::lemmas::phase_refl(before_apply@.nodes@[j].phase);
            }
            if !(event is Fact) && !(event is Reply) { report::lemmas::unchanged(report_before@); }
            if match report::event_target(event) { Some(node) => node.0 >= before_apply@.nodes@.len(), _ => true } {
                report::lemmas::unchanged(report_before@);
            }
        }
        report::lemmas::observe(event, phase_view(before_apply@.nodes@), phase_view(w.nodes@),
            report_input(before_apply@, event), stop_review@, report_before@, report_middle@, report.entries@);
    }
    kernel_proof! {
        if before_apply@.nodes@.len() > 0
            && before_apply@.nodes@[0].phase == Phase::Unplanned
            && progress_unplanned::obstructed(before_apply@.nodes@[0])
            && event == progress_unplanned::outside_event(before_apply@.nodes@[0])
        {
            assert(match event {
                Event::Command { node, .. } | Event::Ack { node, .. } | Event::Fact { node, .. }
                    => node == NodeId(0),
                _ => false,
            });
            assert(w.nodes@.len() == before_apply@.nodes@.len());
            assert forall|j: int| 1 <= j < before_apply@.nodes@.len()
                implies w.nodes@[j] == before_apply@.nodes@[j] by {}
        }
        if !progress_gate_apply::resume_selected(before_apply@, event) {
            reveal(progress_gate_apply::resume_apply_post);
        }
        events_lemmas::footprints_after(before_apply@, *w);
    }
}
}
