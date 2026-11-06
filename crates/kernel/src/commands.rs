//! Executable commands transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
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
        port_acks_specs::command_result(old(w).nodes@[i as int], final(w).nodes@[i as int], id, command),
        port_acks_specs::command_io(old(w).next_question, final(w).next_question, *old(out), *final(out), command),
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, command),
        // #806: forward the delegated Answer contract after assigning its command ID.
        match command {
            Command::Answer { question, answer } =>
                (id > old(w).nodes@[i as int].last_cmd && answers::answer_supported(answer))
                ==> answers::answer_post(
                    Node { last_cmd: id, ..old(w).nodes@[i as int] },
                    final(w).nodes@[i as int], question, answer,
                    old(w).next_question, final(w).next_question),
            _ => true,
        },
        match command {
            Command::Answer { answer, .. } =>
                (answers::answer_supported(answer) && retirement::notices_only(*old(out)))
                ==> retirement::notices_only(*final(out)),
            _ => true,
        },
        (id <= old(w).nodes@[i as int].last_cmd
            ==> *final(w) == *old(w) && *final(out) == *old(out)),

] {
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    kernel_snapshot! { before_command_world = *w }
    kernel_snapshot! { command_out_before = *out }
    kernel_snapshot! { command_question_before = w.next_question }
    if id <= w.nodes[i].last_cmd {
        kernel_proof! { assert(port_acks_specs::command_io(w.next_question, w.next_question, *out, *out, command)) by { reveal(port_acks_specs::command_io); } }
        kernel_proof! { port_acks_commands::duplicate_result(w.nodes@[i as int], id, command);
            reveal(port_acks_specs::pause_post); reveal(port_acks_specs::paused_node); }
        kernel_proof! { reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed); }
        return; // a repeated command
    }
    w.nodes[i].last_cmd = id;
    kernel_proof! { cleanup_lemmas::resource_distance(before_command_node@, w.nodes@[i as int]); }
    kernel_snapshot! { fresh_command_node = w.nodes@[i as int] }
    kernel_proof! { if matches!(command, Command::Stop | Command::ChangeRequest) { port_acks_commands::id_result(before_command_node@, fresh_command_node@, id); } }
    match command {
        Command::Stop => {
            enter_stopping(&mut w.nodes[i], Outcome::Stopped);
            kernel_proof! { port_acks_commands::stopping_result(before_command_node@, fresh_command_node@, w.nodes@[i as int], id, command, Outcome::Stopped); }
        },
        Command::ChangeRequest => {
            enter_stopping(&mut w.nodes[i], Outcome::Replaced);
            kernel_proof! { port_acks_commands::stopping_result(before_command_node@, fresh_command_node@, w.nodes@[i as int], id, command, Outcome::Replaced); }
        },
        Command::Pause => {
            let m = &mut w.nodes[i];
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                m.paused = true;
            }
            kernel_proof! { reveal(port_acks_specs::pause_post); reveal(port_acks_specs::paused_node);
                assert(port_acks_specs::pause_post(before_command_node@, *m, id, command)); }
        }
        Command::Resume => {
            w.nodes[i].paused = false;
            kernel_proof! {
                reveal(progress_gate_apply::resumed); reveal(progress_gate_apply::resume_post);
                assert(progress_gate_apply::resume_post(before_command_node@, w.nodes@[i as int], id, command));
                port_acks_commands::resume_progress_result(before_command_world@, before_command_node@, w.nodes@[i as int], id);
            }
        },
        Command::Answer { question, answer } => {
            kernel_proof! {
                if node_checked(before_command_world@, before_command_node@)
                    && command_progress_input(before_command_node@, command) {
                    port_acks_commands::answer_command_frame(before_command_world@, *w, before_command_node@, id);
                    assert(node_checked(*w, w.nodes@[i as int]));
                    assert(answer_progress_input(w.nodes@[i as int], question, answer));
                    assert(distance(w.nodes@[i as int]) == distance(before_command_node@));
                }
            }
            on_answer(w, i, question, answer, out);
        },
    }
    kernel_proof! {
        if command != Command::Resume { reveal(progress_gate_apply::resume_post); }
        if command != Command::Pause { reveal(port_acks_specs::pause_post); }
        if matches!(command, Command::Stop | Command::ChangeRequest) { port_acks_commands::command_progress_result(before_command_world@, before_command_node@, w.nodes@[i as int], id, command); }
        assert(port_acks_specs::command_io(command_question_before@, w.next_question, command_out_before@, *out, command)) by { reveal(port_acks_specs::command_io); }
        if command == Command::Pause { port_acks_commands::pause_to_command_result(before_command_node@, w.nodes@[i as int], id); }
        if command == Command::Resume { port_acks_commands::resume_result(before_command_node@, w.nodes@[i as int], id); }
        if !port_acks_specs::owner_command(command) { reveal(port_acks_specs::command_result); }
    }
}
}
