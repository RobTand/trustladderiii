//! Executable commands transitions and their #771 safety contracts.
use super::*;
/// on_command lives in command_dispatch (rung 8: its own module, so its proof context
/// holds only what the dispatcher calls); re-exported so callers are unchanged.
pub(super) use super::command_dispatch::on_command;

kernel_fn! {
/// Stop rows / #771: on_command's Stop arm as one world entry (K5 design 3).
pub(super) fn stop_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].credit.withheld == old(w).nodes@[i as int].credit.withheld,
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    let mut m = w.nodes[i];
    m.last_cmd = id;
    m = stopped_v(m, Outcome::Stopped);
    w.nodes[i] = m;
    kernel_proof! {
        assert(w.nodes@ =~= before_rules_world@.nodes@.update(i as int, m));
        apply_command_safety::stop_world(before_rules_world@, *w, i, id);
    }
}
}

kernel_fn! {
/// Replacement rows / #771: on_command's ChangeRequest arm as one world entry (K5 design 3).
pub(super) fn change_request_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].credit.withheld == old(w).nodes@[i as int].credit.withheld,
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    let mut m = w.nodes[i];
    m.last_cmd = id;
    m = stopped_v(m, Outcome::Replaced);
    w.nodes[i] = m;
    kernel_proof! {
        assert(w.nodes@ =~= before_rules_world@.nodes@.update(i as int, m));
        apply_command_safety::change_request_world(before_rules_world@, *w, i, id);
    }
}
}

kernel_fn! {
/// Pause rows (opus 1110): pause_command's node statements as one value step whose only
/// post is the exact node, so the world entry holds no branch.
fn pause_v(n: Node, id: u64) -> (result: Node) [
    ensures result == (if n.phase is Stopping || n.phase is Closed {
            Node { last_cmd: id, ..n }
        } else {
            Node { paused: true, ..Node { last_cmd: id, ..n } }
        }),
] {
    let mut m = n;
    m.last_cmd = id;
    if !m.phase.is_stopping() && !m.phase.is_closed() {
        m.paused = true;
    }
    m
}
}

kernel_fn! {
/// Pause rows / #771: on_command's Pause arm as one world entry (K5 design 3).
pub(super) fn pause_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].credit.withheld == old(w).nodes@[i as int].credit.withheld,
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    let mut m = w.nodes[i];
    m = pause_v(m, id);
    w.nodes[i] = m;
    kernel_proof! {
        assert(w.nodes@ =~= before_rules_world@.nodes@.update(i as int, m));
        apply_command_safety::pause_world(before_rules_world@, *w, i, id);
    }
}
}

kernel_fn! {
/// Resume rows / #771: on_command's Resume arm as one world entry (K5 design 3).
pub(super) fn resume_command(w: &mut World, i: usize, id: u64) -> (result: ()) [
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].credit.withheld == old(w).nodes@[i as int].credit.withheld,
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    kernel_snapshot! { before_command_node = w.nodes@[i as int] }
    kernel_proof! { apply_command_safety::resume_facts(*w, w.nodes@[i as int], id); }
    w.nodes[i].last_cmd = id;
    w.nodes[i].paused = false;
    kernel_proof! {
        handler_safety::step_identity(before_command_node@, w.nodes@[i as int]);
        if retirement::node_rules(before_rules_world@, before_command_node@) {
            handler_safety::rules_world_update(before_rules_world@, *w,
                before_command_node@, w.nodes@[i as int]);
        }
        assert(w.nodes@ =~= before_rules_world@.nodes@.update(i as int, w.nodes@[i as int]));
    }
}
}

kernel_fn! {
/// Answer rows / #771: on_command's Answer arm as one world entry (K5 design 3).
pub(super) fn answer_command(w: &mut World, i: usize, id: u64, question: u64, answer: Answer,
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
        handler_safety::answer_pair(old(w).nodes@[i as int], final(w).nodes@[i as int],
            id > old(w).nodes@[i as int].last_cmd, question, answer, final(w).dependencies@,
            NodeId(i as u64)),
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    let mut m = w.nodes[i];
    m.last_cmd = id;
    w.nodes[i] = m;
    kernel_proof! {
        apply_command_safety::answer_mid(before_rules_world@, *w, i, id, question, answer);
    }
    on_answer(w, i, question, answer, out);
    kernel_proof! {
        apply_command_safety::other_resume_post(before_rules_world@.nodes@[i as int], w.nodes@[i as int],
            id, Command::Answer { question, answer });
        apply_command_safety::answer_pair_last_cmd(before_rules_world@.nodes@[i as int], id,
            w.nodes@[i as int], question, answer, w.dependencies@, NodeId(i as u64));
        apply_command_safety::frame_to_update(before_rules_world@.nodes@, w.nodes@, i as int);
    }
}
}

kernel_fn! {
/// Answer rows / #771: on_command's Answer arm as one world entry (K5 design 3).
pub(super) fn answer_command_lit(w: &mut World, i: usize, id: u64, question: u64, answer: Answer,
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
        handler_safety::answer_pair(old(w).nodes@[i as int], final(w).nodes@[i as int],
            id > old(w).nodes@[i as int].last_cmd, question, answer, final(w).dependencies@,
            NodeId(i as u64)),
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    let mut m = w.nodes[i];
    kernel_snapshot! { read_node = m }
    m.last_cmd = id;
    w.nodes[i] = m;
    kernel_proof! {
        apply_command_safety::answer_mid_lit(before_rules_world@, *w, read_node@, id, question, answer);
    }
    on_answer(w, i, question, answer, out);
    kernel_proof! {
        apply_command_safety::other_resume_post(before_rules_world@.nodes@[i as int], w.nodes@[i as int],
            id, Command::Answer { question, answer });
        apply_command_safety::answer_pair_last_cmd(before_rules_world@.nodes@[i as int], id,
            w.nodes@[i as int], question, answer, w.dependencies@, NodeId(i as u64));
        apply_command_safety::frame_to_update(before_rules_world@.nodes@, w.nodes@, i as int);
    }
}
}

kernel_fn! {
/// Answer rows / #771: on_command's Answer arm as one world entry (K5 design 3).
pub(super) fn answer_command_pa(w: &mut World, i: usize, id: u64, question: u64, answer: Answer,
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
        handler_safety::answer_pair(old(w).nodes@[i as int], final(w).nodes@[i as int],
            id > old(w).nodes@[i as int].last_cmd, question, answer, final(w).dependencies@,
            NodeId(i as u64)),
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

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_rules_world = *w }
    let mut m = w.nodes[i];
    kernel_snapshot! { read_node = m }
    m.last_cmd = id;
    w.nodes[i] = m;
    kernel_proof! {
        apply_command_safety::answer_mid_pa(before_rules_world@, *w, read_node@, m, id, question, answer);
    }
    on_answer(w, i, question, answer, out);
    kernel_proof! {
        apply_command_safety::other_resume_post(before_rules_world@.nodes@[i as int], w.nodes@[i as int],
            id, Command::Answer { question, answer });
        apply_command_safety::answer_pair_last_cmd(before_rules_world@.nodes@[i as int], id,
            w.nodes@[i as int], question, answer, w.dependencies@, NodeId(i as u64));
        apply_command_safety::frame_to_update(before_rules_world@.nodes@, w.nodes@, i as int);
    }
}
}
