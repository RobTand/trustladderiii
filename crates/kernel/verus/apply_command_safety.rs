//! Node facts for the unchanged Apply command updates.
use super::*;

verus! {
/// Resume: every per-node fact resume_command exports, over the exact node update,
/// so the world query never reveals resumed / resume_post.
pub(super) proof fn resume_facts(w: World, b: Node, id: u64)
    requires id > b.last_cmd,
    ensures ({ let r = Node { paused: false, ..Node { last_cmd: id, ..b } };
        progress_gate_apply::resume_post(b, r, id, Command::Resume)
        && handler_safety::command_step(b, r)
        && (node_checked(w, b) && command_progress_input(b, Command::Resume)
            ==> distance(r) < distance(b))
        && (retirement::node_rules(w, b) ==> handler_safety::node_rules_v(r, w.next_question)) }),
{
    let a = Node { last_cmd: id, ..b };
    let r = Node { paused: false, ..a };
    reveal(progress_gate_apply::resumed);
    reveal(progress_gate_apply::resume_post);
    if node_checked(w, b) && command_progress_input(b, Command::Resume) {
        resume_decreases(w, b);
        handler_safety::distance_last_cmd(Node { paused: false, ..b }, id);
    }
    handler_safety::field_step(a, r);
    handler_safety::last_cmd_step(b, a, r);
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, w.next_question, id);
        handler_safety::field_rules_v(a, w.next_question, false);
    }
}

pub(super) proof fn pause_facts(w: World, b: Node, id: u64)
    requires id > b.last_cmd,
    ensures ({ let a = Node { last_cmd: id, ..b };
        let p = Node { paused: true, ..a };
        progress_gate_apply::resume_post(b, a, id, Command::Pause)
        && progress_gate_apply::resume_post(b, p, id, Command::Pause)
        && handler_safety::command_step(b, a)
        && handler_safety::command_step(b, p)
        && !command_progress_input(b, Command::Pause)
        && (retirement::node_rules(w, b) ==> handler_safety::node_rules_v(a, w.next_question)
            && handler_safety::node_rules_v(p, w.next_question)) }),
{
    let a = Node { last_cmd: id, ..b };
    let p = Node { paused: true, ..a };
    reveal(progress_gate_apply::resume_post);
    handler_safety::same_step(a);
    handler_safety::last_cmd_step(b, a, a);
    handler_safety::field_step(a, p);
    handler_safety::last_cmd_step(b, a, p);
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, w.next_question, id);
        handler_safety::field_rules_v(a, w.next_question, true);
    }
}


pub(super) proof fn last_cmd_shift(n: Node, nq: u64, id: u64)
    requires node_headroom(n), arithmetic_consistent(n),
    ensures node_headroom(Node { last_cmd: id, ..n }),
        arithmetic_consistent(Node { last_cmd: id, ..n }),
        handler_safety::node_ok(n, nq) ==> handler_safety::node_ok(Node { last_cmd: id, ..n }, nq),
{
}


pub(super) proof fn answer_command_facts(w: World, b: Node, id: u64, question: u64, answer: Answer)
    requires node_headroom(b), arithmetic_consistent(b), w.next_question < u64::MAX,
        id > b.last_cmd,
    ensures ({ let a = Node { last_cmd: id, ..b };
        let r = apply_answer_values::answered(a, w.next_question, question, answer);
        (node_checked(w, b) && command_progress_input(b, Command::Answer { question, answer })
            ==> distance(r.0) < distance(b))
        && (b.phase is Closed ==> r.0.phase is Closed)
        && r.0.repo == b.repo
        && r.0.footprint == b.footprint
        && (settle_shape(b) ==> settle_shape(r.0))
        && r.0.generation as int <= b.generation as int + 1
        && r.0.next_seq == b.next_seq
        && r.0.credit.start == b.credit.start
        && r.0.credit.earned == b.credit.earned
        && r.0.credit.added as int <= b.credit.added as int + 255
        && r.0.credit.spent as int <= b.credit.spent as int + 1
        && r.0.last_cmd == id
        && r.1 as int <= w.next_question as int + 1
        && w.next_question <= r.1
        && progress_gate_apply::resume_post(b, r.0, id, Command::Answer { question, answer })
        && r.0.proposed == b.proposed
        && r.0.adopted == b.adopted
        && (b.phase is Closed ==> r.0.phase == b.phase)
        && handler_safety::command_step(b, r.0)
        && (node_checked(w, b) && r.0 != (Node { last_cmd: r.0.last_cmd, ..b })
            ==> (match b.question { Some(q) => q.id == question, None => false })
                && (match r.0.question { Some(q) => q.id != question, None => true }))
        && (r.0.holds ==> b.holds)
        && (r.0.pool ==> b.pool)
        && (r.0.ticket == b.ticket || r.0.ticket.is_none())
        && r.0.credit.withheld == b.credit.withheld
        && (retirement::node_rules(w, b) ==> handler_safety::node_rules_v(r.0, r.1)) }),
{
    let a = Node { last_cmd: id, ..b };
    let nq = w.next_question;
    let r = apply_answer_values::answered(a, nq, question, answer);
    last_cmd_shift(b, nq, id);
    apply_answer_safety::answer_frame(a, nq, question, answer);
    apply_answer_safety::answer_resources(a, nq, question, answer);
    apply_answer_safety::answer_question_order(a, nq, question, answer);
    apply_answer_safety::answer_step(a, nq, question, answer);
    handler_safety::last_cmd_step(b, a, r.0);
    reveal(progress_gate_apply::resume_post);
    if node_checked(w, b) {
        handler_safety::checked_node_ok(w, b);
        handler_safety::distance_last_cmd(b, id);
        apply_answer_safety::answer_progress(a, nq, question, answer);
        apply_answer_safety::answer_consumed(a, nq, question, answer);
    }
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, nq, id);
        apply_answer_safety::answer_rules(a, nq, question, answer);
    }
}


pub(super) proof fn nodes_rewrite_frame(s: Seq<Node>, i: int, a: Node, x: Node)
    requires 0 <= i < s.len(),
    ensures s.update(i, a).update(i, x) == s.update(i, x),
{
    assert(s.update(i, a).update(i, x) =~= s.update(i, x));
}


/// Stop world bridge.
/// Opus 1110: stop_command's ensures verbatim over (before, after) from the exact node-table
/// equation, so the entry's query holds no node row derivation under its mutable borrow.
pub(super) proof fn stop_world(before: World, after: World, i: usize, id: u64)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int,
            retirement::stopped(Node { last_cmd: id, ..before.nodes@[i as int] }, Outcome::Stopped)),
        after.next_ticket == before.next_ticket,
        after.next_question == before.next_question,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
    ensures (node_checked(before, before.nodes@[i as int])
            && id > before.nodes@[i as int].last_cmd
            && command_progress_input(before.nodes@[i as int], (Command::Stop))
            ==> distance(after.nodes@[i as int]) < distance(before.nodes@[i as int])),
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        (settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1,
        after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq,
        after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start,
        after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned,
        after.nodes@[i as int].credit.added as int <= before.nodes@[i as int].credit.added as int + 255,
        after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1,
        after.nodes@[i as int].last_cmd == if id > before.nodes@[i as int].last_cmd {
            id
        } else { before.nodes@[i as int].last_cmd },
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        (Command::Stop) == Command::Resume ==> after.next_question == before.next_question,
        progress_gate_apply::resume_post(before.nodes@[i as int], after.nodes@[i as int], id, (Command::Stop)),
        after.nodes@[i as int].proposed == before.nodes@[i as int].proposed,
        after.nodes@[i as int].adopted == before.nodes@[i as int].adopted,
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase == before.nodes@[i as int].phase),
        handler_safety::command_step(before.nodes@[i as int], after.nodes@[i as int]),

        (after.nodes@[i as int].holds ==> before.nodes@[i as int].holds),
        (after.nodes@[i as int].pool ==> before.nodes@[i as int].pool),
        after.nodes@[i as int].ticket == before.nodes@[i as int].ticket || after.nodes@[i as int].ticket.is_none(),
        after.nodes@[i as int].credit.withheld == before.nodes@[i as int].credit.withheld,
        after.nodes@ == before.nodes@.update(i as int, after.nodes@[i as int]),
        before.next_question <= after.next_question,
        (retirement::node_rules(before, before.nodes@[i as int])
            ==> retirement::node_rules(after, after.nodes@[i as int])),
{
    let b = before.nodes@[i as int];
    let a = Node { last_cmd: id, ..b };
    let v = retirement::stopped(a, Outcome::Stopped);
    assert(after.nodes@[i as int] == v);
    allocation_stopping_lemmas::stopping_frame(a, v, Outcome::Stopped);
    handler_safety::stop_step(a, Outcome::Stopped);
    handler_safety::last_cmd_step(b, a, v);
    handler_safety::step_identity(b, v);
    handler_safety::stopped_no_grants(a, Outcome::Stopped);
    reveal(progress_gate_apply::resume_post);
    if retirement::node_rules(before, b) {
        handler_safety::rules_to_v(before, b);
        handler_safety::last_cmd_rules_v(b, before.next_question, id);
        handler_safety::stopped_rules_v(a, Outcome::Stopped, before.next_question);
        handler_safety::rules_world_update(before, after, b, v);
    }
}

/// ChangeRequest world bridge.
/// Opus 1110: change_request_command's ensures verbatim over (before, after) from the exact node-table
/// equation, so the entry's query holds no node row derivation under its mutable borrow.
pub(super) proof fn change_request_world(before: World, after: World, i: usize, id: u64)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int,
            retirement::stopped(Node { last_cmd: id, ..before.nodes@[i as int] }, Outcome::Replaced)),
        after.next_ticket == before.next_ticket,
        after.next_question == before.next_question,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
    ensures (node_checked(before, before.nodes@[i as int])
            && id > before.nodes@[i as int].last_cmd
            && command_progress_input(before.nodes@[i as int], (Command::ChangeRequest))
            ==> distance(after.nodes@[i as int]) < distance(before.nodes@[i as int])),
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        (settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1,
        after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq,
        after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start,
        after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned,
        after.nodes@[i as int].credit.added as int <= before.nodes@[i as int].credit.added as int + 255,
        after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1,
        after.nodes@[i as int].last_cmd == if id > before.nodes@[i as int].last_cmd {
            id
        } else { before.nodes@[i as int].last_cmd },
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        (Command::ChangeRequest) == Command::Resume ==> after.next_question == before.next_question,
        progress_gate_apply::resume_post(before.nodes@[i as int], after.nodes@[i as int], id, (Command::ChangeRequest)),
        after.nodes@[i as int].proposed == before.nodes@[i as int].proposed,
        after.nodes@[i as int].adopted == before.nodes@[i as int].adopted,
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase == before.nodes@[i as int].phase),
        handler_safety::command_step(before.nodes@[i as int], after.nodes@[i as int]),

        (after.nodes@[i as int].holds ==> before.nodes@[i as int].holds),
        (after.nodes@[i as int].pool ==> before.nodes@[i as int].pool),
        after.nodes@[i as int].ticket == before.nodes@[i as int].ticket || after.nodes@[i as int].ticket.is_none(),
        after.nodes@[i as int].credit.withheld == before.nodes@[i as int].credit.withheld,
        after.nodes@ == before.nodes@.update(i as int, after.nodes@[i as int]),
        before.next_question <= after.next_question,
        (retirement::node_rules(before, before.nodes@[i as int])
            ==> retirement::node_rules(after, after.nodes@[i as int])),
{
    let b = before.nodes@[i as int];
    let a = Node { last_cmd: id, ..b };
    let v = retirement::stopped(a, Outcome::Replaced);
    assert(after.nodes@[i as int] == v);
    allocation_stopping_lemmas::stopping_frame(a, v, Outcome::Replaced);
    handler_safety::stop_step(a, Outcome::Replaced);
    handler_safety::last_cmd_step(b, a, v);
    handler_safety::step_identity(b, v);
    handler_safety::stopped_no_grants(a, Outcome::Replaced);
    reveal(progress_gate_apply::resume_post);
    if node_checked(before, b) && command_progress_input(b, Command::ChangeRequest) {
        allocation_stopping_lemmas::stopping_progress(a, v, Outcome::Replaced);
        handler_safety::distance_last_cmd(b, id);
    }
    if retirement::node_rules(before, b) {
        handler_safety::rules_to_v(before, b);
        handler_safety::last_cmd_rules_v(b, before.next_question, id);
        handler_safety::stopped_rules_v(a, Outcome::Replaced, before.next_question);
        handler_safety::rules_world_update(before, after, b, v);
    }
}

/// Pause: the exact node pause_command's unchanged statements leave at index i.
pub(super) open spec fn pause_node(b: Node, id: u64) -> Node {
    if b.phase is Stopping || b.phase is Closed {
        Node { last_cmd: id, ..b }
    } else {
        Node { paused: true, ..Node { last_cmd: id, ..b } }
    }
}

/// Pause world bridge (opus 1110): pause_command's ensures verbatim over (before, after)
/// from the exact node-table equation; pause_facts supplies the opaque node atoms.
pub(super) proof fn pause_world(before: World, after: World, i: usize, id: u64)
    requires i < before.nodes@.len(),
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int, pause_node(before.nodes@[i as int], id)),
        after.next_ticket == before.next_ticket,
        after.next_question == before.next_question,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
    ensures (node_checked(before, before.nodes@[i as int])
            && id > before.nodes@[i as int].last_cmd
            && command_progress_input(before.nodes@[i as int], (Command::Pause))
            ==> distance(after.nodes@[i as int]) < distance(before.nodes@[i as int])),
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        (settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1,
        after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq,
        after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start,
        after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned,
        after.nodes@[i as int].credit.added as int <= before.nodes@[i as int].credit.added as int + 255,
        after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1,
        after.nodes@[i as int].last_cmd == if id > before.nodes@[i as int].last_cmd {
            id
        } else { before.nodes@[i as int].last_cmd },
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        (Command::Pause) == Command::Resume ==> after.next_question == before.next_question,
        progress_gate_apply::resume_post(before.nodes@[i as int], after.nodes@[i as int], id, (Command::Pause)),
        after.nodes@[i as int].proposed == before.nodes@[i as int].proposed,
        after.nodes@[i as int].adopted == before.nodes@[i as int].adopted,
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase == before.nodes@[i as int].phase),
        handler_safety::command_step(before.nodes@[i as int], after.nodes@[i as int]),

        (after.nodes@[i as int].holds ==> before.nodes@[i as int].holds),
        (after.nodes@[i as int].pool ==> before.nodes@[i as int].pool),
        after.nodes@[i as int].ticket == before.nodes@[i as int].ticket || after.nodes@[i as int].ticket.is_none(),
        after.nodes@[i as int].credit.withheld == before.nodes@[i as int].credit.withheld,
        after.nodes@ == before.nodes@.update(i as int, after.nodes@[i as int]),
        before.next_question <= after.next_question,
        (retirement::node_rules(before, before.nodes@[i as int])
            ==> retirement::node_rules(after, after.nodes@[i as int])),
{
    let b = before.nodes@[i as int];
    let a = Node { last_cmd: id, ..b };
    let p = Node { paused: true, ..a };
    pause_facts(before, b, id);
    assert(after.nodes@[i as int] == pause_node(b, id));
    if b.phase is Stopping || b.phase is Closed {
        assert(after.nodes@[i as int] == a);
        handler_safety::step_identity(b, a);
        if retirement::node_rules(before, b) {
            handler_safety::rules_world_update(before, after, b, a);
        }
    } else {
        assert(after.nodes@[i as int] == p);
        handler_safety::step_identity(b, p);
        if retirement::node_rules(before, b) {
            handler_safety::rules_world_update(before, after, b, p);
        }
    }
}

}
