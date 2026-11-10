//! Executable events transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// #771: compose the actual event handlers' per-apply counter bounds.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
#[allow(clippy::collapsible_if)]
pub(super) fn apply_proved(w: &mut World, event: Event, out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures report::agrees(event,
            phase_view(old(w).nodes@), phase_view(final(w).nodes@), report_input(*old(w), event),
            old(report).entries@, final(report).entries@,
            (phase_view(final(w).nodes@), final(report).entries@)),
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
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, id, command } => node.0 < old(w).nodes@.len() ==>
                (match command {
                    Command::Answer { question, answer } => handler_safety::answer_pair(
                        old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int],
                        id > old(w).nodes@[node.0 as int].last_cmd, question, answer,
                        final(w).dependencies@, node),
                    _ => handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int]),
                })
                && transition_safety::notice_only(*old(out), *final(out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event(event) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), event), transition_safety::funding_due(*old(w), event),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(out), *final(out)),
            _ => true,
        }),
        (match event {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
        (match event {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply(event), transition_safety::emitted(*old(out), *final(out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply(event), transition_safety::emitted(*old(out), *final(out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
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
                kernel_proof! { apply_new_safety::adoption_c2(before_apply@, *w, repo); }
            }
            kernel_proof! {
                transition_safety::lift_apply_none(before_apply@, *w, event, before_apply_out@, *out);
            }
        }
        Event::NewRequest { repo, priority } => apply_new_request(w, repo, priority, out),
        Event::Reply {
            node,
            generation,
            reply,
        } => apply_reply(w, node, generation, reply, out, report),
        Event::Ack { node, seq, ack } => apply_ack(w, node, seq, ack, out),
        Event::Fact { node, fact } => apply_fact(w, node, fact, out, report),
        Event::Command { node, id, command } => apply_command(w, node, id, command, out),
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
        assert(phase_view(before_apply@.nodes@).len() <= phase_view(w.nodes@).len() <= phase_view(before_apply@.nodes@).len() + 1);
        assert forall|j: int| phase_view(before_apply@.nodes@).len() <= j < phase_view(w.nodes@).len()
            implies !(phase_view(w.nodes@)[j] is Stopping || phase_view(w.nodes@)[j] is Closed) by {}
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
        if event is NewAdoption {
            events_lemmas::footprints_after(before_apply@, *w);
        }
    }
}
}

kernel_fn! {
/// K5 DEV ONLY (fv-k5-int-dev-1110), CONTRACTS.md E1: TRUSTED export wrapper. Contract =
/// `apply_proved`'s, verbatim, plus one opaque export clause (v2.10; R1-R5 via apply_exports_rows), E10's Ack clause (v2.31) and E1.5 (v2.35). The body only calls the
/// verified `apply_proved`: no new executable behaviour. Never lands; escape_audit FAILS here.
#[cfg_attr(verus_keep_ghost, verifier::external_body)]
pub(super) fn apply(w: &mut World, event: Event, out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures report::agrees(event,
            phase_view(old(w).nodes@), phase_view(final(w).nodes@), report_input(*old(w), event),
            old(report).entries@, final(report).entries@,
            (phase_view(final(w).nodes@), final(report).entries@)),
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
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, id, command } => node.0 < old(w).nodes@.len() ==>
                (match command {
                    Command::Answer { question, answer } => handler_safety::answer_pair(
                        old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int],
                        id > old(w).nodes@[node.0 as int].last_cmd, question, answer,
                        final(w).dependencies@, node),
                    _ => handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int]),
                })
                && transition_safety::notice_only(*old(out), *final(out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event(event) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), event), transition_safety::funding_due(*old(w), event),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(out), *final(out)),
            _ => true,
        }),
        (match event {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
        (match event {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply(event), transition_safety::emitted(*old(out), *final(out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply(event), transition_safety::emitted(*old(out), *final(out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
        }),
    transition_safety::apply_exports(*old(w), *final(w), event, *old(out), *final(out)),
    // E10 (CONTRACTS.md v2.31; TRUSTED-FROZEN, owner debt): the Ack arm forwards on_ack's opaque post.
    (match event {
        Event::Ack { node, seq, ack } => node.0 < old(w).nodes@.len()
            ==> acknowledgements::on_ack_post(*old(w), *final(w), node.0 as int, seq, ack, *old(out), *final(out)),
        _ => true,
    }),
    // E1.5 (CONTRACTS.md v2.35; TRUSTED-FROZEN, owner apply): the Command rows, opaque; consumers use
    // command_frames_reveal::command_rows_project.
    command_frames::command_rows(*old(w), *final(w), event),
] {
    apply_proved(w, event, out, report)
}
}

kernel_fn! {
/// G9 dispatcher entry / #806: apply's NewRequest arm as one world entry.
fn apply_new_request(w: &mut World, repo: RepoId, priority: Priority,
    _out: &mut Outbox) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(_out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures (progress_input(*old(w), (Event::NewRequest { repo, priority }))
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
            && (Event::NewRequest { repo, priority }) == progress_unplanned::outside_event(old(w).nodes@[0]))
            ==> final(w).nodes@.len() == old(w).nodes@.len()
                && (forall|j: int| 1 <= j < old(w).nodes@.len()
                    ==> final(w).nodes@[j] == old(w).nodes@[j]),
        match (Event::NewRequest { repo, priority }) {
            Event::Fact { node, fact } => retirement::selected(fact)
                && node.0 < old(w).nodes@.len() ==> {
                    final(w).nodes@ == old(w).nodes@.update(node.0 as int,
                        retirement::fact_node(old(w).nodes@[node.0 as int], fact))
                    && final(w).next_question == old(w).next_question
                    && final(w).file_sets@ == old(w).file_sets@
                    && final(w).dependencies@ == old(w).dependencies@
                    && (retirement::notices_only(*old(_out)) ==> retirement::notices_only(*final(_out)))
                    && match fact {
                        Fact::Priority(_) => *final(_out) == *old(_out),
                        Fact::PrMerged { head, checks_passed } => old(w).nodes@[node.0 as int].had_pr
                            && old(w).nodes@[node.0 as int].merged.is_none()
                            && !(old(w).nodes@[node.0 as int].phase is Closed)
                            && !(old(w).nodes@[node.0 as int].approved == Some(head) && checks_passed)
                            ==> retirement::unapproved_note(*final(_out), node.0 as usize),
                        _ => true,
                    }
                },
            _ => true,
        },
        progress_gate_apply::resume_apply_post(*old(w), *final(w), (Event::NewRequest { repo, priority })),
        (match (Event::NewRequest { repo, priority }) {
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, command, .. } => node.0 < old(w).nodes@.len() ==>
                handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int])
                && transition_safety::notice_only(*old(_out), *final(_out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event((Event::NewRequest { repo, priority })) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), (Event::NewRequest { repo, priority })), transition_safety::funding_due(*old(w), (Event::NewRequest { repo, priority })),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(_out), *final(_out)),
            _ => true,
        }),
        (match (Event::NewRequest { repo, priority }) {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(_out), *final(_out)),
            _ => true,
        }),
        (match (Event::NewRequest { repo, priority }) {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::NewRequest { repo, priority })), transition_safety::emitted(*old(_out), *final(_out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::NewRequest { repo, priority })), transition_safety::emitted(*old(_out), *final(_out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
        }),
        forall|j: int| 0 <= j < old(w).nodes@.len() ==> final(w).nodes@[j] == old(w).nodes@[j],
        forall|j: int| old(w).nodes@.len() <= j < final(w).nodes@.len()
            ==> !(final(w).nodes@[j].phase is Stopping || final(w).nodes@[j].phase is Closed),
        *final(_out) == *old(_out),

        final(w).dependencies@ == old(w).dependencies@,
        old(w).next_question <= final(w).next_question,
        (repo.0 < old(w).gates@.len() ==> {
            let n = final(w).nodes@[old(w).nodes@.len() as int];
            retirement::node_rules(*final(w), n) && !n.holds && !n.pool
                && n.ticket.is_none() && !n.credit.withheld
        }),
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table); reveal(progress_input); reveal(fresh_commands);] {
    kernel_snapshot! { before_apply = *w }
    kernel_snapshot! { before_apply_out = *_out }
    kernel_proof! { report_projection(*w, (Event::NewRequest { repo, priority })); events_lemmas::selected_input(before_apply@, (Event::NewRequest { repo, priority })); }
        if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
            let mut node = Node::new(repo);
            node.priority = priority;
            w.nodes.push(node);
            kernel_proof! {
                handler_safety::initial_rules(*w, w.nodes@[before_apply@.nodes@.len() as int], repo);
            }
        }
        kernel_proof! {
            transition_safety::lift_apply_none(before_apply@, *w, (Event::NewRequest { repo, priority }), before_apply_out@, *_out);
        }
    kernel_proof! { events_lemmas::footprints_after(before_apply@, *w); if !progress_gate_apply::resume_selected(before_apply@, (Event::NewRequest { repo, priority })) { reveal(progress_gate_apply::resume_apply_post); } if before_apply@.nodes@.len() > 0 && before_apply@.nodes@[0].phase == Phase::Unplanned && progress_unplanned::obstructed(before_apply@.nodes@[0]) && (Event::NewRequest { repo, priority }) == progress_unplanned::outside_event(before_apply@.nodes@[0]) { assert(w.nodes@.len() == before_apply@.nodes@.len()); assert forall|j: int| 1 <= j < before_apply@.nodes@.len() implies w.nodes@[j] == before_apply@.nodes@[j] by {} } }
}
}

kernel_fn! {
/// G9 dispatcher entry / #806: apply's Reply arm as one world entry (claims no G9 relation).
fn apply_reply(w: &mut World, node: NodeId, generation: u64, reply: Reply,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures (progress_input(*old(w), (Event::Reply { node, generation, reply }))
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
            && (Event::Reply { node, generation, reply }) == progress_unplanned::outside_event(old(w).nodes@[0]))
            ==> final(w).nodes@.len() == old(w).nodes@.len()
                && (forall|j: int| 1 <= j < old(w).nodes@.len()
                    ==> final(w).nodes@[j] == old(w).nodes@[j]),
        match (Event::Reply { node, generation, reply }) {
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
        progress_gate_apply::resume_apply_post(*old(w), *final(w), (Event::Reply { node, generation, reply })),
        (match (Event::Reply { node, generation, reply }) {
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, command, .. } => node.0 < old(w).nodes@.len() ==>
                handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int])
                && transition_safety::notice_only(*old(out), *final(out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event((Event::Reply { node, generation, reply })) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), (Event::Reply { node, generation, reply })), transition_safety::funding_due(*old(w), (Event::Reply { node, generation, reply })),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Reply { node, generation, reply }) {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Reply { node, generation, reply }) {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Reply { node, generation, reply })), transition_safety::emitted(*old(out), *final(out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Reply { node, generation, reply })), transition_safety::emitted(*old(out), *final(out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
        }),
        final(w).nodes@.len() == old(w).nodes@.len(),
        node.0 < old(w).nodes@.len() ==> transition_safety::others_unchanged(*old(w), *final(w), node.0 as int),
        node.0 >= old(w).nodes@.len() ==> final(w).nodes@ == old(w).nodes@,
        node.0 < old(w).nodes@.len() ==> report::forwards(old(report).entries@, final(report).entries@,
            report::event_entries((Event::Reply { node, generation, reply }), report_input(*old(w), (Event::Reply { node, generation, reply })))),
        node.0 >= old(w).nodes@.len() ==> final(report).entries@ == old(report).entries@,
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table); reveal(progress_input); reveal(fresh_commands);] {
    kernel_snapshot! { before_apply = *w }
    kernel_snapshot! { before_apply_out = *out }
    kernel_proof! { report_projection(*w, (Event::Reply { node, generation, reply })); events_lemmas::selected_input(before_apply@, (Event::Reply { node, generation, reply })); report::lemmas::entries_for_event((Event::Reply { node, generation, reply }), report_input(*w, (Event::Reply { node, generation, reply }))); }
        if let Some(i) = w.index(node) {
            on_reply(w, i, generation, reply, out, report);
            kernel_proof! {
                if progress_input(before_apply@, (Event::Reply { node, generation, reply })) {
                    assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                    progress::lift_node_progress(before_apply@, *w, i as int);
                    dependencies::indexability_frame(before_apply@, *w);
                }
            }
        }
    kernel_proof! { events_lemmas::footprints_after(before_apply@, *w); if !progress_gate_apply::resume_selected(before_apply@, (Event::Reply { node, generation, reply })) { reveal(progress_gate_apply::resume_apply_post); } if before_apply@.nodes@.len() > 0 && before_apply@.nodes@[0].phase == Phase::Unplanned && progress_unplanned::obstructed(before_apply@.nodes@[0]) && (Event::Reply { node, generation, reply }) == progress_unplanned::outside_event(before_apply@.nodes@[0]) { assert(w.nodes@.len() == before_apply@.nodes@.len()); assert forall|j: int| 1 <= j < before_apply@.nodes@.len() implies w.nodes@[j] == before_apply@.nodes@[j] by {} } }
}
}

kernel_fn! {
/// G9 dispatcher entry / #806: apply's Ack arm as one world entry (claims no G9 relation).
fn apply_ack(w: &mut World, node: NodeId, seq: u64, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures (progress_input(*old(w), (Event::Ack { node, seq, ack }))
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
            && (Event::Ack { node, seq, ack }) == progress_unplanned::outside_event(old(w).nodes@[0]))
            ==> final(w).nodes@.len() == old(w).nodes@.len()
                && (forall|j: int| 1 <= j < old(w).nodes@.len()
                    ==> final(w).nodes@[j] == old(w).nodes@[j]),
        match (Event::Ack { node, seq, ack }) {
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
        progress_gate_apply::resume_apply_post(*old(w), *final(w), (Event::Ack { node, seq, ack })),
        (match (Event::Ack { node, seq, ack }) {
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, command, .. } => node.0 < old(w).nodes@.len() ==>
                handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int])
                && transition_safety::notice_only(*old(out), *final(out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event((Event::Ack { node, seq, ack })) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), (Event::Ack { node, seq, ack })), transition_safety::funding_due(*old(w), (Event::Ack { node, seq, ack })),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Ack { node, seq, ack }) {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Ack { node, seq, ack }) {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Ack { node, seq, ack })), transition_safety::emitted(*old(out), *final(out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Ack { node, seq, ack })), transition_safety::emitted(*old(out), *final(out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
        }),
        final(w).nodes@.len() == old(w).nodes@.len(),
        node.0 < old(w).nodes@.len() ==> transition_safety::others_unchanged(*old(w), *final(w), node.0 as int),
        node.0 >= old(w).nodes@.len() ==> final(w).nodes@ == old(w).nodes@,
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table); reveal(progress_input); reveal(fresh_commands);] {
    kernel_snapshot! { before_apply = *w }
    kernel_snapshot! { before_apply_out = *out }
    kernel_proof! { report_projection(*w, (Event::Ack { node, seq, ack })); events_lemmas::selected_input(before_apply@, (Event::Ack { node, seq, ack })); }
        if let Some(i) = w.index(node) {
            on_ack(w, i, seq, ack, out);
            kernel_proof! {
                if progress_input(before_apply@, (Event::Ack { node, seq, ack })) {
                    assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                    progress::lift_node_progress(before_apply@, *w, i as int);
                    dependencies::indexability_frame(before_apply@, *w);
                }
            }
        }
    kernel_proof! { events_lemmas::footprints_after(before_apply@, *w); if !progress_gate_apply::resume_selected(before_apply@, (Event::Ack { node, seq, ack })) { reveal(progress_gate_apply::resume_apply_post); } if before_apply@.nodes@.len() > 0 && before_apply@.nodes@[0].phase == Phase::Unplanned && progress_unplanned::obstructed(before_apply@.nodes@[0]) && (Event::Ack { node, seq, ack }) == progress_unplanned::outside_event(before_apply@.nodes@[0]) { assert(w.nodes@.len() == before_apply@.nodes@.len()); assert forall|j: int| 1 <= j < before_apply@.nodes@.len() implies w.nodes@[j] == before_apply@.nodes@[j] by {} } }
}
}

kernel_fn! {
/// G9 dispatcher entry / #806: apply's Fact arm as one world entry (claims no G9 relation).
fn apply_fact(w: &mut World, node: NodeId, fact: Fact,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures (progress_input(*old(w), (Event::Fact { node, fact }))
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
            && (Event::Fact { node, fact }) == progress_unplanned::outside_event(old(w).nodes@[0]))
            ==> final(w).nodes@.len() == old(w).nodes@.len()
                && (forall|j: int| 1 <= j < old(w).nodes@.len()
                    ==> final(w).nodes@[j] == old(w).nodes@[j]),
        match (Event::Fact { node, fact }) {
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
        progress_gate_apply::resume_apply_post(*old(w), *final(w), (Event::Fact { node, fact })),
        (match (Event::Fact { node, fact }) {
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, command, .. } => node.0 < old(w).nodes@.len() ==>
                handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int])
                && transition_safety::notice_only(*old(out), *final(out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event((Event::Fact { node, fact })) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), (Event::Fact { node, fact })), transition_safety::funding_due(*old(w), (Event::Fact { node, fact })),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Fact { node, fact }) {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Fact { node, fact }) {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Fact { node, fact })), transition_safety::emitted(*old(out), *final(out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Fact { node, fact })), transition_safety::emitted(*old(out), *final(out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
        }),
        final(w).nodes@.len() == old(w).nodes@.len(),
        node.0 < old(w).nodes@.len() ==> transition_safety::others_unchanged(*old(w), *final(w), node.0 as int),
        node.0 >= old(w).nodes@.len() ==> final(w).nodes@ == old(w).nodes@,
        final(w).nodes@.len() == old(w).nodes@.len(),
        node.0 < old(w).nodes@.len() ==> report::fact_report(old(report).entries@, final(report).entries@, node,
            report_input(*old(w), (Event::Fact { node, fact })).phase, report_input(*old(w), (Event::Fact { node, fact })).current,
            final(w).nodes@[node.0 as int].phase, fact),
        node.0 >= old(w).nodes@.len() ==> final(report).entries@ == old(report).entries@,
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table); reveal(progress_input); reveal(fresh_commands);] {
    kernel_snapshot! { before_apply = *w }
    kernel_snapshot! { before_apply_out = *out }
    kernel_proof! { report_projection(*w, (Event::Fact { node, fact })); events_lemmas::selected_input(before_apply@, (Event::Fact { node, fact })); }
        if let Some(i) = w.index(node) {
            on_fact(w, i, fact, out, report);
        }
    kernel_proof! { events_lemmas::footprints_after(before_apply@, *w); if !progress_gate_apply::resume_selected(before_apply@, (Event::Fact { node, fact })) { reveal(progress_gate_apply::resume_apply_post); } if before_apply@.nodes@.len() > 0 && before_apply@.nodes@[0].phase == Phase::Unplanned && progress_unplanned::obstructed(before_apply@.nodes@[0]) && (Event::Fact { node, fact }) == progress_unplanned::outside_event(before_apply@.nodes@[0]) { assert(w.nodes@.len() == before_apply@.nodes@.len()); assert forall|j: int| 1 <= j < before_apply@.nodes@.len() implies w.nodes@[j] == before_apply@.nodes@[j] by {} } }
}
}

kernel_fn! {
/// G9 dispatcher entry / #806: apply's Command arm as one world entry (G9 relation under node_checked).
fn apply_command(w: &mut World, node: NodeId, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures (progress_input(*old(w), (Event::Command { node, id, command }))
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
            && (Event::Command { node, id, command }) == progress_unplanned::outside_event(old(w).nodes@[0]))
            ==> final(w).nodes@.len() == old(w).nodes@.len()
                && (forall|j: int| 1 <= j < old(w).nodes@.len()
                    ==> final(w).nodes@[j] == old(w).nodes@[j]),
        match (Event::Command { node, id, command }) {
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
        progress_gate_apply::resume_apply_post(*old(w), *final(w), (Event::Command { node, id, command })),
        (match (Event::Command { node, id, command }) {
            Event::NewAdoption { repo, head, .. } => repo.0 < old(w).gates@.len() ==>
                final(w).nodes@.len() == old(w).nodes@.len() + 1
                && final(w).nodes@[old(w).nodes@.len() as int].proposed == Some(head)
                && !final(w).nodes@[old(w).nodes@.len() as int].adopted,
            Event::Command { node, id, command } => node.0 < old(w).nodes@.len() ==>
                (match command {
                    Command::Answer { question, answer } => handler_safety::answer_pair(
                        old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int],
                        id > old(w).nodes@[node.0 as int].last_cmd, question, answer,
                        final(w).dependencies@, node),
                    _ => handler_safety::command_step(old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int]),
                })
                && transition_safety::notice_only(*old(out), *final(out), node)
                && (match command {
                    Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[node.0 as int])
                        && final(w).nodes@[node.0 as int]
                            != (Node { last_cmd: final(w).nodes@[node.0 as int].last_cmd, ..old(w).nodes@[node.0 as int] })
                        ==> (match old(w).nodes@[node.0 as int].question {
                            Some(q) => q.id == question,
                            None => false,
                        }) && (match final(w).nodes@[node.0 as int].question {
                            Some(q) => q.id != question,
                            None => true,
                        }),
                    _ => true,
                }),
            Event::Reply { node, generation, reply } => node.0 < old(w).nodes@.len()
                && transition_safety::fail_fast_event((Event::Command { node, id, command })) ==> handler_safety::reply_rows(old(w).fail_fast,
                    transition_safety::retry_due(*old(w), (Event::Command { node, id, command })), transition_safety::funding_due(*old(w), (Event::Command { node, id, command })),
                    old(w).nodes@[node.0 as int], final(w).nodes@[node.0 as int], *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Command { node, id, command }) {
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees } =>
                old(w).nodes@.len() > 0 ==> port_t860_contracts::reply_post(*old(w), *final(w), 0,
                    generation, Reply::AuditDisagrees, *old(out), *final(out)),
            _ => true,
        }),
        (match (Event::Command { node, id, command }) {
            Event::NewRequest { .. } | Event::NewAdoption { .. } =>
                transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Command { node, id, command })), transition_safety::emitted(*old(out), *final(out))),
            Event::Command { node, .. } => node.0 < old(w).nodes@.len()
                && node_checked(*old(w), old(w).nodes@[node.0 as int])
                ==> transition_safety::transition_safety(*old(w), *final(w),
                    transition_safety::Transition::Apply((Event::Command { node, id, command })), transition_safety::emitted(*old(out), *final(out))),
            Event::Reply { .. } | Event::Ack { .. } | Event::Fact { .. } => true,
        }),
        final(w).nodes@.len() == old(w).nodes@.len(),
        node.0 < old(w).nodes@.len() ==> transition_safety::others_unchanged(*old(w), *final(w), node.0 as int),
        node.0 >= old(w).nodes@.len() ==> final(w).nodes@ == old(w).nodes@,

        final(w).dependencies@ == old(w).dependencies@,
        old(w).next_question <= final(w).next_question,
        old(w).nodes@.len() <= u64::MAX,
        (node.0 < old(w).nodes@.len() ==> {
            let b = old(w).nodes@[node.0 as int];
            let n = final(w).nodes@[node.0 as int];
            (match command {
                Command::Answer { .. } => true,  // N1 out for Answer (lead ruling 11-10; S2 open gap)
                _ => retirement::node_rules(*old(w), b) ==> retirement::node_rules(*final(w), n),
            })
                && (n.holds ==> b.holds) && (n.pool ==> b.pool)
                && (n.ticket == b.ticket || n.ticket.is_none())
                && n.repo == b.repo && n.footprint == b.footprint
                && final(w).nodes@ == old(w).nodes@.update(node.0 as int, n)
        }),
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table); reveal(progress_input); reveal(fresh_commands);] {
    kernel_snapshot! { before_apply = *w }
    kernel_snapshot! { before_apply_out = *out }
    kernel_proof! { report_projection(*w, (Event::Command { node, id, command })); events_lemmas::selected_input(before_apply@, (Event::Command { node, id, command })); }
        if let Some(i) = w.index(node) {
            on_command(w, i, id, command, out);
            kernel_proof! {
                if node_checked(before_apply@, before_apply@.nodes@[i as int]) {
                    handler_safety::command_node_safety(before_apply@, *w, (Event::Command { node, id, command }), i as int,
                        before_apply_out@, *out);
                    transition_safety::lift_apply(before_apply@, *w, (Event::Command { node, id, command }), i as int,
                        before_apply_out@, *out);
                }
                if command == Command::Resume {
                    progress_gate_apply_resume_frame::compose(before_apply@, *w, node, id);
                }
                if progress_input(before_apply@, (Event::Command { node, id, command })) {
                    assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                    progress::lift_node_progress(before_apply@, *w, i as int);
                    dependencies::indexability_frame(before_apply@, *w);
                }
            }
        }
    kernel_proof! { events_lemmas::footprints_after(before_apply@, *w); if !progress_gate_apply::resume_selected(before_apply@, (Event::Command { node, id, command })) { reveal(progress_gate_apply::resume_apply_post); } if before_apply@.nodes@.len() > 0 && before_apply@.nodes@[0].phase == Phase::Unplanned && progress_unplanned::obstructed(before_apply@.nodes@[0]) && (Event::Command { node, id, command }) == progress_unplanned::outside_event(before_apply@.nodes@[0]) { assert(w.nodes@.len() == before_apply@.nodes@.len()); assert forall|j: int| 1 <= j < before_apply@.nodes@.len() implies w.nodes@[j] == before_apply@.nodes@[j] by {} } }
}
}
