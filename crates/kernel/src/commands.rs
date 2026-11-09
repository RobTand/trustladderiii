//! Executable commands transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
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
        handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
] {
    kernel_snapshot! { before_command = w.nodes@[i as int] }
    kernel_proof! {
        if command == Command::Resume && id > w.nodes@[i as int].last_cmd
            && command_progress_input(w.nodes@[i as int], command)
            && node_checked(*w, w.nodes@[i as int]) { resume_decreases(*w, w.nodes@[i as int]); }
    }
    command_node(&mut w.nodes[i], &mut w.next_question, i, id, command, out);
    kernel_proof! {
        if command == Command::Resume && id > before_command@.last_cmd {
            assert(distance(w.nodes@[i as int]) == distance(Node { paused: false, ..before_command@ }))
                by { reveal(distance); }
        }
    }
}
}

kernel_fn! {
/// Command rows / #771: on_command's node work, verified against one node so
/// the world frame is proved once at the call.
fn command_node(m: &mut Node, next_question: &mut u64, i: usize, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires old(out).len < OUTBOX,
        node_headroom(*old(m)),
        arithmetic_consistent(*old(m)),
        *old(next_question) < u64::MAX,
    ensures (id > old(m).last_cmd && command != Command::Resume
            && command_progress_input(*old(m), command)
            ==> distance(*final(m)) < distance(*old(m))),
        (command == Command::Resume && id > old(m).last_cmd
            ==> *final(m) == (Node { paused: false, last_cmd: id, ..*old(m) })),
        (old(m).phase is Closed ==> final(m).phase is Closed),
        final(m).repo == old(m).repo,
        final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq,
        final(m).credit.start == old(m).credit.start,
        final(m).credit.earned == old(m).credit.earned,
        final(m).credit.added as int <= old(m).credit.added as int + 255,
        final(m).credit.spent as int <= old(m).credit.spent as int + 1,
        final(m).last_cmd == if id > old(m).last_cmd { id } else { old(m).last_cmd },
        *final(next_question) as int <= *old(next_question) as int + 1,
        final(out).len <= old(out).len + 1,
        command == Command::Resume ==> *final(next_question) == *old(next_question),
        progress_gate_apply::resume_post(*old(m), *final(m), id, command),
        final(m).proposed == old(m).proposed,
        final(m).adopted == old(m).adopted,
        (old(m).phase is Closed ==> final(m).phase == old(m).phase),
        handler_safety::command_step(*old(m), *final(m)),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);] {
    if id <= m.last_cmd {
        kernel_proof! { reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed); }
        kernel_proof! {
            handler_safety::same_step(*m);
            handler_safety::step_identity(*m, *m);
        }
        return; // a repeated command
    }
    kernel_snapshot! { before_command_node = *m }
    m.last_cmd = id;
    kernel_snapshot! { after_id = *m }
    match command {
        Command::Stop => enter_stopping(m, Outcome::Stopped),
        Command::ChangeRequest => enter_stopping(m, Outcome::Replaced),
        Command::Pause => {
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                m.paused = true;
            }
        }
        Command::Resume => {
            m.paused = false;
            kernel_proof! {
                reveal(progress_gate_apply::resumed); reveal(progress_gate_apply::resume_post);
                assert(progress_gate_apply::resume_post(before_command_node@, *m, id, command));
            }
        },
        Command::Answer { question, answer } => {
            kernel_proof! {
                if command_progress_input(before_command_node@, command) {
                    assert(answer_progress_input(*m, question, answer));
                    assert(distance(*m) == distance(before_command_node@));
                }
            }
            answer_node(m, next_question, i, question, answer, out);
        },
    }
    kernel_proof! {
        match command {
            Command::Stop => handler_safety::stop_step(after_id@, Outcome::Stopped),
            Command::ChangeRequest => handler_safety::stop_step(after_id@, Outcome::Replaced),
            Command::Answer { .. } => {},
            _ => handler_safety::field_step(after_id@, *m),
        }
        handler_safety::last_cmd_step(before_command_node@, after_id@, *m);
        handler_safety::step_identity(before_command_node@, *m);
    }
    kernel_proof! {
        if command != Command::Resume { reveal(progress_gate_apply::resume_post); }
    }
}
}
