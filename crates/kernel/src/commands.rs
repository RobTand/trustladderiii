//! Executable commands transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
/// K5 design 3: a pure dispatcher; each arm is one world entry with this contract.
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
        (match command {
            Command::Answer { question, answer } => ({
                let b = old(w).nodes@[i as int];
                let n = final(w).nodes@[i as int];
                handler_safety::command_step(b, Node {
                    credit: Credit { withheld: b.credit.withheld, ..n.credit }, ..n
                }) && n.credit.withheld == (if id > b.last_cmd
                    && b.phase == Phase::AwaitingRob && (match b.question {
                        Some(q) => q.id == question && (match (q.kind, answer) {
                            (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue)
                                | (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
                            _ => false,
                        }),
                        None => false,
                    }) {
                    dependencies::withheld_after(b.credit.withheld, n.planning_started,
                        n.phase, n.credit.ready, final(w).dependencies@, NodeId(i as u64))
                } else { b.credit.withheld })
            }),
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
] {
    if id <= w.nodes[i].last_cmd {
        kernel_proof! { reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed); }
        kernel_proof! {
            handler_safety::same_step(w.nodes@[i as int]);
            handler_safety::step_identity(w.nodes@[i as int], w.nodes@[i as int]);
        }
        return; // a repeated command
    }
    match command {
        Command::Stop => stop_command(w, i, id),
        Command::ChangeRequest => change_request_command(w, i, id),
        Command::Pause => pause_command(w, i, id),
        Command::Resume => resume_command(w, i, id),
        Command::Answer { question, answer } => answer_command(w, i, id, question, answer, out),
    }
}
}

kernel_fn! {
/// Stop rows / #771: on_command's Stop arm as one world entry (K5 design 3).
fn stop_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        id > old(w).nodes@[i as int].last_cmd,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], (Command::Stop))
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
        (Command::Stop) == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, (Command::Stop)),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
] {
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    w.nodes[i].last_cmd = id;
    kernel_snapshot! { after_id = w.nodes@[i as int] }
    enter_stopping(&mut w.nodes[i], Outcome::Stopped);
    kernel_proof! {
        handler_safety::stop_step(after_id@, Outcome::Stopped);
        reveal(progress_gate_apply::resume_post);
    }
    kernel_proof! {
        handler_safety::last_cmd_step(before_command_node@, after_id@, w.nodes@[i as int]);
        handler_safety::step_identity(before_command_node@, w.nodes@[i as int]);
    }
}
}

kernel_fn! {
/// Replacement rows / #771: on_command's ChangeRequest arm as one world entry (K5 design 3).
fn change_request_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        id > old(w).nodes@[i as int].last_cmd,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], (Command::ChangeRequest))
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
        (Command::ChangeRequest) == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, (Command::ChangeRequest)),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
] {
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    w.nodes[i].last_cmd = id;
    kernel_snapshot! { after_id = w.nodes@[i as int] }
    kernel_proof! {
        assert(distance(after_id@) == distance(before_command_node@)) by { reveal(distance); }
    }
    enter_stopping(&mut w.nodes[i], Outcome::Replaced);
    kernel_proof! {
        handler_safety::stop_step(after_id@, Outcome::Replaced);
        reveal(progress_gate_apply::resume_post);
    }
    kernel_proof! {
        handler_safety::last_cmd_step(before_command_node@, after_id@, w.nodes@[i as int]);
        handler_safety::step_identity(before_command_node@, w.nodes@[i as int]);
    }
}
}

kernel_fn! {
/// Pause rows / #771: on_command's Pause arm as one world entry (K5 design 3).
fn pause_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        id > old(w).nodes@[i as int].last_cmd,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], (Command::Pause))
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
        (Command::Pause) == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, (Command::Pause)),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
] {
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    w.nodes[i].last_cmd = id;
    kernel_snapshot! { after_id = w.nodes@[i as int] }
    let m = &mut w.nodes[i];
    if !m.phase.is_stopping() && !m.phase.is_closed() {
        m.paused = true;
    }
    kernel_proof! {
        handler_safety::field_step(after_id@, w.nodes@[i as int]);
        reveal(progress_gate_apply::resume_post);
    }
    kernel_proof! {
        handler_safety::last_cmd_step(before_command_node@, after_id@, w.nodes@[i as int]);
        handler_safety::step_identity(before_command_node@, w.nodes@[i as int]);
    }
}
}

kernel_fn! {
/// Resume rows / #771: on_command's Resume arm as one world entry (K5 design 3).
fn resume_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        id > old(w).nodes@[i as int].last_cmd,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], (Command::Resume))
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
        (Command::Resume) == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, (Command::Resume)),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
] {
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    kernel_proof! {
        if command_progress_input(w.nodes@[i as int], Command::Resume)
            && node_checked(*w, w.nodes@[i as int]) { resume_decreases(*w, w.nodes@[i as int]); }
    }
    w.nodes[i].last_cmd = id;
    kernel_snapshot! { after_id = w.nodes@[i as int] }
    w.nodes[i].paused = false;
    kernel_proof! {
        reveal(progress_gate_apply::resumed); reveal(progress_gate_apply::resume_post);
        assert(progress_gate_apply::resume_post(before_command_node@, w.nodes@[i as int], id, Command::Resume));
        handler_safety::distance_last_cmd(Node { paused: false, ..before_command_node@ }, id);
        handler_safety::field_step(after_id@, w.nodes@[i as int]);
    }
    kernel_proof! {
        handler_safety::last_cmd_step(before_command_node@, after_id@, w.nodes@[i as int]);
        handler_safety::step_identity(before_command_node@, w.nodes@[i as int]);
    }
}
}

kernel_fn! {
/// Answer rows / #771: on_command's Answer arm as one world entry (K5 design 3).
fn answer_command(w: &mut World, i: usize, id: u64, question: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        id > old(w).nodes@[i as int].last_cmd,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && id > old(w).nodes@[i as int].last_cmd
            && command_progress_input(old(w).nodes@[i as int], (Command::Answer { question, answer }))
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
        (Command::Answer { question, answer }) == Command::Resume ==> final(w).next_question == old(w).next_question,
        progress_gate_apply::resume_post(old(w).nodes@[i as int], final(w).nodes@[i as int], id, (Command::Answer { question, answer })),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        ({
            let b = old(w).nodes@[i as int];
            let n = final(w).nodes@[i as int];
            handler_safety::command_step(b, Node {
                credit: Credit { withheld: b.credit.withheld, ..n.credit }, ..n
            }) && n.credit.withheld == (if id > b.last_cmd
                && b.phase == Phase::AwaitingRob && (match b.question {
                    Some(q) => q.id == question && (match (q.kind, answer) {
                        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue)
                            | (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
                        _ => false,
                    }),
                    None => false,
                }) {
                dependencies::withheld_after(b.credit.withheld, n.planning_started,
                    n.phase, n.credit.ready, final(w).dependencies@, NodeId(i as u64))
            } else { b.credit.withheld })
        }),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
        (match (Command::Answer { question, answer }) {
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
] {
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    kernel_snapshot! { before_command_world = *w }
    w.nodes[i].last_cmd = id;
    kernel_snapshot! { after_id = w.nodes@[i as int] }
    kernel_proof! {
        if node_checked(before_command_world@, before_command_node@) {
            footprint::valid_under_table_growth(before_command_world@, *w, w.nodes@[i as int].footprint);
            assert(node_checked(*w, w.nodes@[i as int]));
            if command_progress_input(before_command_node@, Command::Answer { question, answer }) {
                assert(answer_progress_input(w.nodes@[i as int], question, answer));
                assert(distance(after_id@) == distance(before_command_node@)) by { reveal(distance); }
            }
        }
    }
    on_answer(w, i, question, answer, out);
    kernel_proof! {
        reveal(progress_gate_apply::resume_post);
    }
    kernel_proof! {
        let answered = Node {
            credit: Credit {
                withheld: before_command_node@.credit.withheld,
                ..w.nodes@[i as int].credit
            },
            ..w.nodes@[i as int]
        };
        handler_safety::last_cmd_step(before_command_node@, after_id@, answered);
        handler_safety::step_identity(before_command_node@, answered);
    }
}
}
