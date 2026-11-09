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
    kernel_snapshot! { before_command = w.nodes@[i as int] }
    kernel_proof! {
        if node_checked(*w, w.nodes@[i as int]) { handler_safety::checked_node_ok(*w, w.nodes@[i as int]); }
        if command == Command::Resume && id > w.nodes@[i as int].last_cmd
            && command_progress_input(w.nodes@[i as int], command)
            && node_checked(*w, w.nodes@[i as int]) { resume_decreases(*w, w.nodes@[i as int]); }
    }
    let (n, nq, note) = command_node(w.nodes[i], w.next_question, id, command);
    w.nodes[i] = n;
    w.next_question = nq;
    match note {
        Some(x) => notice(out, i, x),
        None => {}
    }
    kernel_proof! {
        if command == Command::Resume && id > before_command@.last_cmd {
            assert(distance(w.nodes@[i as int]) == distance(Node { paused: false, ..before_command@ }))
                by { reveal(distance); }
        }
    }
}
}

kernel_fn! {
/// Command rows / #771: on_command's node work as a value function (K5): the
/// node and next question id come in by value; the commanded node, the next id
/// and at most one notice are returned for on_command to write back.
fn command_node(n: Node, nq: u64, id: u64, command: Command)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (handler_safety::node_ok(n, nq) && id > n.last_cmd && command != Command::Resume
            && command_progress_input(n, command)
            ==> distance(result.0) < distance(n)),
        (command == Command::Resume && id > n.last_cmd
            ==> result.0 == (Node { paused: false, last_cmd: id, ..n })),
        (n.phase is Closed ==> result.0.phase is Closed),
        result.0.repo == n.repo,
        result.0.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result.0)),
        result.0.generation as int <= n.generation as int + 1,
        result.0.next_seq == n.next_seq,
        result.0.credit.start == n.credit.start,
        result.0.credit.earned == n.credit.earned,
        result.0.credit.added as int <= n.credit.added as int + 255,
        result.0.credit.spent as int <= n.credit.spent as int + 1,
        result.0.last_cmd == if id > n.last_cmd { id } else { n.last_cmd },
        result.1 as int <= nq as int + 1,
        command == Command::Resume ==> result.1 == nq,
        progress_gate_apply::resume_post(n, result.0, id, command),
        result.0.proposed == n.proposed,
        result.0.adopted == n.adopted,
        (n.phase is Closed ==> result.0.phase == n.phase),
        handler_safety::command_step(n, result.0),
        (match command {
            Command::Answer { question, .. } => handler_safety::node_ok(n, nq)
                && result.0 != (Node { last_cmd: result.0.last_cmd, ..n })
                ==> (match n.question {
                    Some(q) => q.id == question,
                    None => false,
                }) && (match result.0.question {
                    Some(q) => q.id != question,
                    None => true,
                }),
            _ => true,
        }),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    if id <= m.last_cmd {
        kernel_proof! { reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed); }
        kernel_proof! {
            handler_safety::same_step(m);
            handler_safety::step_identity(m, m);
        }
        return (m, nq, note); // a repeated command
    }
    kernel_snapshot! { before_command_node = m }
    m.last_cmd = id;
    kernel_snapshot! { after_id = m }
    kernel_proof! {
        assert(distance(m) == distance(before_command_node@)) by { reveal(distance); }
    }
    match command {
        Command::Stop => m = enter_stopping_v(m, Outcome::Stopped),
        Command::ChangeRequest => m = enter_stopping_v(m, Outcome::Replaced),
        Command::Pause => {
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                m.paused = true;
            }
        }
        Command::Resume => {
            m.paused = false;
            kernel_proof! {
                reveal(progress_gate_apply::resumed); reveal(progress_gate_apply::resume_post);
                assert(progress_gate_apply::resume_post(before_command_node@, m, id, command));
            }
        },
        Command::Answer { question, answer } => {
            kernel_proof! {
                if command_progress_input(before_command_node@, command) {
                    assert(answer_progress_input(m, question, answer));
                    assert(distance(m) == distance(before_command_node@));
                }
                if handler_safety::node_ok(before_command_node@, nq) {
                    assert(handler_safety::node_ok(m, nq));
                }
            }
            let (m1, nq1, x) = answer_node(m, nq, question, answer);
            m = m1;
            nq = nq1;
            note = x;
        },
    }
    kernel_proof! {
        match command {
            Command::Stop => handler_safety::stop_step(after_id@, Outcome::Stopped),
            Command::ChangeRequest => handler_safety::stop_step(after_id@, Outcome::Replaced),
            Command::Answer { .. } => {},
            _ => handler_safety::field_step(after_id@, m),
        }
        handler_safety::last_cmd_step(before_command_node@, after_id@, m);
        handler_safety::step_identity(before_command_node@, m);
    }
    kernel_proof! {
        if command != Command::Resume { reveal(progress_gate_apply::resume_post); }
    }
    (m, nq, note)
}
}
