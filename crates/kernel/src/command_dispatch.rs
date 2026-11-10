//! The command dispatcher on_command, in its own module so that its SMT context holds only
//! what it calls (the five command entries), not what the entries call.
use super::*;

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
/// K5 design 3: a pure dispatcher; each arm is one world entry with this contract.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn on_command_proved(w: &mut World, i: usize, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], command)
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
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == if id > old(w).nodes@[i as int].last_cmd {
            id
        } else { old(w).nodes@[i as int].last_cmd },
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        command == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, command),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        (match command {
            Command::Answer { question, answer } => handler_safety::answer_pair(
                old(w).nodes@[i as int], final(w).nodes@[i as int], id > old(w).nodes@[i as int].last_cmd,
                question, answer, final(w).dependencies@, NodeId(i as u64)),
            _ => handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
        }),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
        (match command {
            Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[i as int])
                && final(w).nodes@[i as int] != (Node { last_cmd: final(w).nodes@[i as int].last_cmd, ..old(w).nodes@[i as int] })
                ==> (match old(w).nodes@[i as int].question {
                    Some(q) => q.id == question,
                    None => false,
                }) && (match final(w).nodes@[i as int].question {
                    Some(q) => q.id != question,
                    None => true,
                }),
            _ => true,
        }),

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        // R28-Answer (E2 framing addition, 11-10): the incident row on the answered node.
        (match command {
            Command::Answer { .. } => retirement::node_rules(*old(w), old(w).nodes@[i as int])
                ==> (final(w).nodes@[i as int].incident_needs_rob ==> final(w).nodes@[i as int].waiting_incident),
            _ => true,
        }),
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }

    if id <= w.nodes[i].last_cmd {
        kernel_proof! { reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed); }
        kernel_proof! {
            handler_safety::same_step(w.nodes@[i as int]);
            handler_safety::step_identity(w.nodes@[i as int], w.nodes@[i as int]);
            match command {
                Command::Answer { question, answer } => {
                    handler_safety::answer_pair_same(
                        w.nodes@[i as int], question, answer, w.dependencies@, NodeId(i as u64));
                    apply_command_safety::rules_incident(*w, w.nodes@[i as int]);
                }
                _ => {}
            }
        }
        kernel_proof! { assert(w.nodes@ =~= before_rules_world@.nodes@.update(i as int, w.nodes@[i as int])); }
        return; // a repeated command
    }
    match command {
        Command::Stop => stop_command(w, i, id),
        Command::ChangeRequest => change_request_command(w, i, id),
        Command::Pause => pause_command(w, i, id),
        Command::Resume => resume_command(w, i, id),
        Command::Answer { question, answer } => answer_command(w, i, id, question, answer, out),
    }
    kernel_proof! {
        match command {
            Command::Answer { .. } => apply_command_safety::answer_frame(before_rules_world@, *w, i),
            _ => assert(w.nodes@ =~= before_rules_world@.nodes@.update(i as int, w.nodes@[i as int])),
        }
    }
}
}

kernel_fn! {
/// K5 DEV ONLY (fv-k5-int-dev-1110), CONTRACTS.md E11.1: TRUSTED export wrapper. Contract =
/// `on_command_proved`'s, verbatim, plus the frozen E11 clauses (direct). The body only calls the
/// verified `on_command_proved`: no new executable behaviour. Never lands; escape_audit FAILS here.
#[cfg_attr(verus_keep_ghost, verifier::external_body)]
pub(super) fn on_command(w: &mut World, i: usize, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], command)
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
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == if id > old(w).nodes@[i as int].last_cmd {
            id
        } else { old(w).nodes@[i as int].last_cmd },
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        command == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, command),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        (match command {
            Command::Answer { question, answer } => handler_safety::answer_pair(
                old(w).nodes@[i as int], final(w).nodes@[i as int], id > old(w).nodes@[i as int].last_cmd,
                question, answer, final(w).dependencies@, NodeId(i as u64)),
            _ => handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
        }),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
        (match command {
            Command::Answer { question, .. } => node_checked(*old(w), old(w).nodes@[i as int])
                && final(w).nodes@[i as int] != (Node { last_cmd: final(w).nodes@[i as int].last_cmd, ..old(w).nodes@[i as int] })
                ==> (match old(w).nodes@[i as int].question {
                    Some(q) => q.id == question,
                    None => false,
                }) && (match final(w).nodes@[i as int].question {
                    Some(q) => q.id != question,
                    None => true,
                }),
            _ => true,
        }),

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        // R28-Answer (E2 framing addition, 11-10): the incident row on the answered node.
        (match command {
            Command::Answer { .. } => retirement::node_rules(*old(w), old(w).nodes@[i as int])
                ==> (final(w).nodes@[i as int].incident_needs_rob ==> final(w).nodes@[i as int].waiting_incident),
            _ => true,
        }),
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
    // E11.1 REQ R24: a fresh Stop of a live item.
    (command is Stop && id > old(w).nodes@[i as int].last_cmd && !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping))
        ==> final(w).nodes@[i as int].phase == Phase::Stopping(Outcome::Stopped) && final(w).nodes@[i as int].pos == 0
            && final(w).nodes@[i as int].ticket.is_none() && final(w).nodes@[i as int].question.is_none() && !final(w).nodes@[i as int].waiting_incident
            && final(w).nodes@[i as int].holds == old(w).nodes@[i as int].holds && final(w).nodes@[i as int].pool == old(w).nodes@[i as int].pool && final(w).nodes@[i as int].run == old(w).nodes@[i as int].run && final(w).nodes@[i as int].job == old(w).nodes@[i as int].job && final(w).nodes@[i as int].pr_open == old(w).nodes@[i as int].pr_open && final(w).nodes@[i as int].worktree == old(w).nodes@[i as int].worktree
            && final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq && final(w).nodes@[i as int].last_cmd == id
            && (match old(w).nodes@[i as int].effect {
                None => final(w).nodes@[i as int].effect.is_none(),
                Some(e) => final(w).nodes@[i as int].effect.is_some() && final(w).nodes@[i as int].effect.unwrap().seq == e.seq
                    && final(w).nodes@[i as int].effect.unwrap().kind == e.kind && final(w).nodes@[i as int].effect.unwrap().cancelling,
            }),
    // E2 framing (v2.35, form rule 2, owner apply): the v2.7 Stop counter/credit/generation rows, E11.1b,
    // E2.N4a and E2.N4b behind one opaque predicate; consumers use command_frames_reveal. E2.N4c stays off
    // (FALSE AS FROZEN, v2.34).
    // v2.40 premise (lead's sweep): stated only for an item consistent before (what the kernel guarantees).
    retirement::node_rules(*old(w), old(w).nodes@[i as int])
        ==> command_frames::command_frame_rows(old(w).nodes@[i as int], final(w).nodes@[i as int], id, command),
] {
    on_command_proved(w, i, id, command, out)
}
}
