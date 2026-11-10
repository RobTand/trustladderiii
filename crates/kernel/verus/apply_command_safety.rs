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



pub(super) proof fn nodes_rewrite_frame(s: Seq<Node>, i: int, a: Node, x: Node)
    requires 0 <= i < s.len(),
    ensures s.update(i, a).update(i, x) == s.update(i, x),
{
    assert(s.update(i, a).update(i, x) =~= s.update(i, x));
}


/// Opus 1110: command_step ignores last_cmd. The premise names the written node directly,
/// so no whole-Node equation enters the caller's query (last_cmd_step's premise does).
pub(super) proof fn last_cmd_rebase(b: Node, id: u64, n: Node)
    requires handler_safety::command_step(Node { last_cmd: id, ..b }, n),
    ensures handler_safety::command_step(b, n),
{
    reveal(handler_safety::command_step);
}

/// Opus 1110: resume_post holds for every command other than Resume.
pub(super) proof fn other_resume_post(b: Node, n: Node, id: u64, command: Command)
    requires command != Command::Resume,
    ensures progress_gate_apply::resume_post(b, n, id, command),
{
    reveal(progress_gate_apply::resume_post);
}

/// Stop, ChangeRequest: field rows of the exact stopped node after the last_cmd write.
pub(super) proof fn stop_rows(b: Node, id: u64, o: Outcome)
    requires node_headroom(b),
    ensures ({ let v = retirement::stopped(Node { last_cmd: id, ..b }, o);
        v.repo == b.repo && v.footprint == b.footprint
        && (settle_shape(b) ==> settle_shape(v))
        && v.generation as int <= b.generation as int + 1
        && v.next_seq == b.next_seq
        && v.credit == b.credit
        && v.last_cmd == id
        && (b.phase is Closed ==> v.phase == b.phase)
        && (v.holds ==> b.holds)
        && (v.pool ==> b.pool)
        && (v.ticket == b.ticket || v.ticket.is_none()) }),
{
    reveal(retirement::stopped);
}

/// Stop, ChangeRequest: the command_step and resume_post rows.
pub(super) proof fn stop_step_rows(b: Node, id: u64, o: Outcome, command: Command)
    requires command != Command::Resume,
    ensures ({ let v = retirement::stopped(Node { last_cmd: id, ..b }, o);
        handler_safety::command_step(b, v)
        && v.proposed == b.proposed
        && v.adopted == b.adopted
        && progress_gate_apply::resume_post(b, v, id, command) }),
{
    let v = retirement::stopped(Node { last_cmd: id, ..b }, o);
    handler_safety::stop_step(Node { last_cmd: id, ..b }, o);
    last_cmd_rebase(b, id, v);
    handler_safety::step_identity(b, v);
    other_resume_post(b, v, id, command);
}

/// Stop, ChangeRequest: the N1 node row.
pub(super) proof fn stop_rules(w: World, b: Node, id: u64, o: Outcome)
    requires o != Outcome::Merged,
    ensures retirement::node_rules(w, b) ==> handler_safety::node_rules_v(
        retirement::stopped(Node { last_cmd: id, ..b }, o), w.next_question),
{
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, w.next_question, id);
        handler_safety::stopped_rules_v(Node { last_cmd: id, ..b }, o, w.next_question);
    }
}

/// ChangeRequest: the progress row.
pub(super) proof fn change_request_progress(w: World, b: Node, id: u64)
    ensures node_checked(w, b) && command_progress_input(b, Command::ChangeRequest)
        ==> distance(retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Replaced)) < distance(b),
{
    if node_checked(w, b) && command_progress_input(b, Command::ChangeRequest) {
        allocation_stopping_lemmas::stopping_progress(Node { last_cmd: id, ..b },
            retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Replaced), Outcome::Replaced);
        handler_safety::distance_last_cmd(b, id);
    }
}

/// Answer: field rows of the answered node after the last_cmd write, against b.
pub(super) proof fn answer_rows(b: Node, nq: u64, id: u64, question: u64, answer: Answer)
    requires node_headroom(b), arithmetic_consistent(b), nq < u64::MAX,
    ensures ({ let r = apply_answer_values::answered(Node { last_cmd: id, ..b }, nq, question, answer);
        (b.phase is Closed ==> r.0.phase is Closed)
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
        && r.1 as int <= nq as int + 1
        && nq <= r.1
        && r.0.proposed == b.proposed
        && r.0.adopted == b.adopted
        && (b.phase is Closed ==> r.0.phase == b.phase)
        && (r.0.holds ==> b.holds)
        && (r.0.pool ==> b.pool)
        && (r.0.ticket == b.ticket || r.0.ticket.is_none())
        && r.0.credit.withheld == b.credit.withheld }),
{
    let a = Node { last_cmd: id, ..b };
    apply_answer_safety::answer_frame(a, nq, question, answer);
    apply_answer_safety::answer_resources(a, nq, question, answer);
    apply_answer_safety::answer_question_order(a, nq, question, answer);
}

/// Answer: the command_step and resume_post rows.
pub(super) proof fn answer_step_rows(b: Node, nq: u64, id: u64, question: u64, answer: Answer)
    requires node_headroom(b), arithmetic_consistent(b), nq < u64::MAX,
    ensures ({ let r = apply_answer_values::answered(Node { last_cmd: id, ..b }, nq, question, answer);
        handler_safety::command_step(b, r.0)
        && progress_gate_apply::resume_post(b, r.0, id, Command::Answer { question, answer }) }),
{
    let a = Node { last_cmd: id, ..b };
    let r = apply_answer_values::answered(a, nq, question, answer);
    apply_answer_safety::answer_step(a, nq, question, answer);
    last_cmd_rebase(b, id, r.0);
    other_resume_post(b, r.0, id, Command::Answer { question, answer });
}

/// Answer: the two node_checked rows (progress, consumed question).
pub(super) proof fn answer_checked_rows(w: World, b: Node, id: u64, question: u64, answer: Answer)
    requires node_headroom(b), arithmetic_consistent(b), w.next_question < u64::MAX,
    ensures ({ let r = apply_answer_values::answered(Node { last_cmd: id, ..b }, w.next_question, question, answer);
        (node_checked(w, b) && command_progress_input(b, Command::Answer { question, answer })
            ==> distance(r.0) < distance(b))
        && (node_checked(w, b) && r.0 != (Node { last_cmd: r.0.last_cmd, ..b })
            ==> (match b.question { Some(q) => q.id == question, None => false })
                && (match r.0.question { Some(q) => q.id != question, None => true })) }),
{
    let a = Node { last_cmd: id, ..b };
    let nq = w.next_question;
    if node_checked(w, b) {
        handler_safety::checked_node_ok(w, b);
        last_cmd_shift(b, nq, id);
        handler_safety::distance_last_cmd(b, id);
        apply_answer_safety::answer_frame(a, nq, question, answer);
        apply_answer_safety::answer_progress(a, nq, question, answer);
        apply_answer_safety::answer_consumed(a, nq, question, answer);
    }
}

/// Answer: the N1 node row.
pub(super) proof fn answer_rules_row(w: World, b: Node, id: u64, question: u64, answer: Answer)
    requires node_headroom(b), arithmetic_consistent(b), w.next_question < u64::MAX,
    ensures ({ let r = apply_answer_values::answered(Node { last_cmd: id, ..b }, w.next_question, question, answer);
        retirement::node_rules(w, b) ==> handler_safety::node_rules_v(r.0, r.1) }),
{
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, w.next_question, id);
        apply_answer_safety::answer_rules(Node { last_cmd: id, ..b }, w.next_question, question, answer);
    }
}

/// Opus 1110: one node written in the table; the four table clauses of every command entry.
pub(super) proof fn world_rewrite(before: World, after: World, i: usize, v: Node)
    requires i < before.nodes@.len(),
        after.nodes@ == before.nodes@.update(i as int, v),
    ensures after.nodes@[i as int] == v,
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@ == before.nodes@.update(i as int, after.nodes@[i as int]),
{
}

/// Stop, ChangeRequest: the entry's equality and bound rows, verbatim over (b, n).
pub(super) proof fn stop_rows_eq(b: Node, n: Node, id: u64, o: Outcome)
    requires n == retirement::stopped(Node { last_cmd: id, ..b }, o),
        node_headroom(b),
    ensures n.repo == b.repo,
        n.footprint == b.footprint,
        n.generation as int <= b.generation as int + 1,
        n.next_seq == b.next_seq,
        n.credit.start == b.credit.start,
        n.credit.earned == b.credit.earned,
        n.credit.added as int <= b.credit.added as int + 255,
        n.credit.spent as int <= b.credit.spent as int + 1,
        n.last_cmd == if id > b.last_cmd {
            id
        } else { b.last_cmd },
        n.credit.withheld == b.credit.withheld,
{
    stop_rows(b, id, o);
}

/// Stop, ChangeRequest: the entry's implication and opaque rows, verbatim over (b, n).
pub(super) proof fn stop_rows_imp(b: Node, n: Node, id: u64, o: Outcome, command: Command)
    requires n == retirement::stopped(Node { last_cmd: id, ..b }, o),
        node_headroom(b),
        command != Command::Resume,
    ensures (b.phase is Closed ==> n.phase is Closed),
        (settle_shape(b) ==> settle_shape(n)),
        progress_gate_apply::resume_post(b, n, id, command),
        n.proposed == b.proposed,
        n.adopted == b.adopted,
        (b.phase is Closed ==> n.phase == b.phase),
        handler_safety::command_step(b, n),
        (n.holds ==> b.holds),
        (n.pool ==> b.pool),
        n.ticket == b.ticket || n.ticket.is_none(),
{
    stop_rows(b, id, o);
    stop_step_rows(b, id, o, command);
}

/// Stop: the progress row is vacuous (Stop is never a progress input).
pub(super) proof fn stop_dist(w: World, b: Node, n: Node, id: u64)
    ensures (node_checked(w, b)
            && id > b.last_cmd
            && command_progress_input(b, (Command::Stop))
            ==> distance(n) < distance(b)),
{
}

/// ChangeRequest: the progress row, verbatim over (w, b, n).
pub(super) proof fn change_request_dist(w: World, b: Node, n: Node, id: u64)
    requires n == retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Replaced),
    ensures (node_checked(w, b)
            && id > b.last_cmd
            && command_progress_input(b, (Command::ChangeRequest))
            ==> distance(n) < distance(b)),
{
    change_request_progress(w, b, id);
}

/// Answer: the entry's equality and bound rows, verbatim over (b, n).
pub(super) proof fn answer_rows_eq(b: Node, n: Node, nq: u64, id: u64, question: u64, answer: Answer)
    requires n == apply_answer_values::answered(Node { last_cmd: id, ..b }, nq, question, answer).0,
        node_headroom(b), arithmetic_consistent(b), nq < u64::MAX,
    ensures n.repo == b.repo,
        n.footprint == b.footprint,
        n.generation as int <= b.generation as int + 1,
        n.next_seq == b.next_seq,
        n.credit.start == b.credit.start,
        n.credit.earned == b.credit.earned,
        n.credit.added as int <= b.credit.added as int + 255,
        n.credit.spent as int <= b.credit.spent as int + 1,
        n.last_cmd == if id > b.last_cmd {
            id
        } else { b.last_cmd },
        n.credit.withheld == b.credit.withheld,
{
    answer_rows(b, nq, id, question, answer);
}

/// Answer: the entry's implication and opaque rows, verbatim over (b, n).
pub(super) proof fn answer_rows_imp(b: Node, n: Node, nq: u64, id: u64, question: u64, answer: Answer)
    requires n == apply_answer_values::answered(Node { last_cmd: id, ..b }, nq, question, answer).0,
        node_headroom(b), arithmetic_consistent(b), nq < u64::MAX,
    ensures (b.phase is Closed ==> n.phase is Closed),
        (settle_shape(b) ==> settle_shape(n)),
        progress_gate_apply::resume_post(b, n, id, (Command::Answer { question, answer })),
        n.proposed == b.proposed,
        n.adopted == b.adopted,
        (b.phase is Closed ==> n.phase == b.phase),
        handler_safety::command_step(b, n),
        (n.holds ==> b.holds),
        (n.pool ==> b.pool),
        n.ticket == b.ticket || n.ticket.is_none(),
{
    answer_rows(b, nq, id, question, answer);
    answer_step_rows(b, nq, id, question, answer);
}

/// Answer: the progress and consumed-question rows, verbatim over (w, b, n).
pub(super) proof fn answer_dist(w: World, b: Node, n: Node, id: u64, question: u64, answer: Answer)
    requires n == apply_answer_values::answered(Node { last_cmd: id, ..b }, w.next_question, question, answer).0,
        node_headroom(b), arithmetic_consistent(b), w.next_question < u64::MAX,
    ensures (node_checked(w, b)
            && id > b.last_cmd
            && command_progress_input(b, (Command::Answer { question, answer }))
            ==> distance(n) < distance(b)),
        (match (Command::Answer { question, answer }) {
            Command::Answer { question, .. } => node_checked(w, b)
                && n != (Node { last_cmd: n.last_cmd, ..b })
                ==> (match b.question {
                    Some(q) => q.id == question,
                    None => false,
                }) && (match n.question {
                    Some(q) => q.id != question,
                    None => true,
                }),
            _ => true,
        }),
{
    answer_checked_rows(w, b, id, question, answer);
}

/// Opus 1110: stop_command's ensures verbatim over (before, after). Every clause is a literal
/// post of world_rewrite, stop_rows_eq, stop_rows_imp, stop_dist or rules_world_update.
pub(super) proof fn stop_world(before: World, after: World, i: usize, id: u64)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int,
            retirement::stopped(Node { last_cmd: id, ..before.nodes@[i as int] }, Outcome::Stopped)),
        after.next_question == before.next_question,
        after.next_ticket == before.next_ticket,
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
    world_rewrite(before, after, i, retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped));
    let n = after.nodes@[i as int];
    stop_rows_eq(b, n, id, Outcome::Stopped);
    stop_rows_imp(b, n, id, Outcome::Stopped, Command::Stop);
    stop_dist(before, b, n, id);
    stop_rules(before, b, id, Outcome::Stopped);
    if retirement::node_rules(before, b) {
        handler_safety::rules_world_update(before, after, b, n);
    }
}

/// Opus 1110: change_request_command's ensures verbatim over (before, after). Every clause is a literal
/// post of world_rewrite, stop_rows_eq, stop_rows_imp, change_request_dist or rules_world_update.
pub(super) proof fn change_request_world(before: World, after: World, i: usize, id: u64)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int,
            retirement::stopped(Node { last_cmd: id, ..before.nodes@[i as int] }, Outcome::Replaced)),
        after.next_question == before.next_question,
        after.next_ticket == before.next_ticket,
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
    world_rewrite(before, after, i, retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Replaced));
    let n = after.nodes@[i as int];
    stop_rows_eq(b, n, id, Outcome::Replaced);
    stop_rows_imp(b, n, id, Outcome::Replaced, Command::ChangeRequest);
    change_request_dist(before, b, n, id);
    stop_rules(before, b, id, Outcome::Replaced);
    if retirement::node_rules(before, b) {
        handler_safety::rules_world_update(before, after, b, n);
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

/// Opus 1110: pause_command's ensures verbatim over (before, after) from the exact
/// node-table equation; pause_facts supplies the opaque node atoms.
pub(super) proof fn pause_world(before: World, after: World, i: usize, id: u64)
    requires i < before.nodes@.len(),
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int, pause_node(before.nodes@[i as int], id)),
        after.next_question == before.next_question,
        after.next_ticket == before.next_ticket,
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

/// Opus 1110: answer_command's ensures verbatim over (before, after, ob, oa) from the exact
/// node-table equation; node rows come from the four answer_* row lemmas.
pub(super) proof fn answer_world(before: World, after: World, i: usize, id: u64, question: u64,
    answer: Answer, ob: Outbox, oa: Outbox)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        arithmetic_consistent(before.nodes@[i as int]),
        before.next_question < u64::MAX,
        id > before.nodes@[i as int].last_cmd,
        after.nodes@ == before.nodes@.update(i as int, apply_answer_values::answered(
            Node { last_cmd: id, ..before.nodes@[i as int] }, before.next_question, question, answer).0),
        after.next_question == apply_answer_values::answered(
            Node { last_cmd: id, ..before.nodes@[i as int] }, before.next_question, question, answer).1,
        after.next_ticket == before.next_ticket,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        oa.len <= ob.len + 1,
        transition_safety::notice_only(ob, oa, NodeId(i as u64)),
    ensures (node_checked(before, before.nodes@[i as int])
            && id > before.nodes@[i as int].last_cmd
            && command_progress_input(before.nodes@[i as int], (Command::Answer { question, answer }))
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
        oa.len <= ob.len + 1,
        (Command::Answer { question, answer }) == Command::Resume ==> after.next_question == before.next_question,
        progress_gate_apply::resume_post(before.nodes@[i as int], after.nodes@[i as int], id, (Command::Answer { question, answer })),
        after.nodes@[i as int].proposed == before.nodes@[i as int].proposed,
        after.nodes@[i as int].adopted == before.nodes@[i as int].adopted,
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase == before.nodes@[i as int].phase),
        handler_safety::command_step(before.nodes@[i as int], after.nodes@[i as int]),
        transition_safety::notice_only(ob, oa, NodeId(i as u64)),
        (match (Command::Answer { question, answer }) {
            Command::Answer { question, .. } => node_checked(before, before.nodes@[i as int])
                && after.nodes@[i as int] != (Node { last_cmd: after.nodes@[i as int].last_cmd, ..before.nodes@[i as int] })
                ==> (match before.nodes@[i as int].question {
                    Some(q) => q.id == question,
                    None => false,
                }) && (match after.nodes@[i as int].question {
                    Some(q) => q.id != question,
                    None => true,
                }),
            _ => true,
        }),

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
    let r = apply_answer_values::answered(Node { last_cmd: id, ..b }, before.next_question, question, answer);
    world_rewrite(before, after, i, r.0);
    let n = after.nodes@[i as int];
    answer_rows(b, before.next_question, id, question, answer);
    answer_rows_eq(b, n, before.next_question, id, question, answer);
    answer_rows_imp(b, n, before.next_question, id, question, answer);
    answer_dist(before, b, n, id, question, answer);
    answer_rules_row(before, b, id, question, answer);
    if retirement::node_rules(before, b) {
        handler_safety::rules_world_update(before, after, b, n);
    }
}

/// Opus 1110: on_command's ensures verbatim for a repeated command id, which changes nothing.
pub(super) proof fn command_stutter(before: World, after: World, i: usize, id: u64, command: Command,
    ob: Outbox, oa: Outbox)
    requires after == before, oa == ob,
        i < before.nodes@.len(),
        id <= before.nodes@[i as int].last_cmd,
    ensures (node_checked(before, before.nodes@[i as int])
            && id > before.nodes@[i as int].last_cmd
            && command_progress_input(before.nodes@[i as int], command)
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
        oa.len <= ob.len + 1,
        command == Command::Resume ==> after.next_question == before.next_question,
        progress_gate_apply::resume_post(before.nodes@[i as int], after.nodes@[i as int], id, command),
        after.nodes@[i as int].proposed == before.nodes@[i as int].proposed,
        after.nodes@[i as int].adopted == before.nodes@[i as int].adopted,
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase == before.nodes@[i as int].phase),
        handler_safety::command_step(before.nodes@[i as int], after.nodes@[i as int]),
        transition_safety::notice_only(ob, oa, NodeId(i as u64)),
        (match command {
            Command::Answer { question, .. } => node_checked(before, before.nodes@[i as int])
                && after.nodes@[i as int] != (Node { last_cmd: after.nodes@[i as int].last_cmd, ..before.nodes@[i as int] })
                ==> (match before.nodes@[i as int].question {
                    Some(q) => q.id == question,
                    None => false,
                }) && (match after.nodes@[i as int].question {
                    Some(q) => q.id != question,
                    None => true,
                }),
            _ => true,
        }),

        (after.nodes@[i as int].holds ==> before.nodes@[i as int].holds),
        (after.nodes@[i as int].pool ==> before.nodes@[i as int].pool),
        after.nodes@[i as int].ticket == before.nodes@[i as int].ticket || after.nodes@[i as int].ticket.is_none(),
        after.nodes@[i as int].credit.withheld == before.nodes@[i as int].credit.withheld,
        after.nodes@ == before.nodes@.update(i as int, after.nodes@[i as int]),
        before.next_question <= after.next_question,
        (retirement::node_rules(before, before.nodes@[i as int])
            ==> retirement::node_rules(after, after.nodes@[i as int])),
{
    reveal(progress_gate_apply::resume_post);
    reveal(progress_gate_apply::resumed);
    handler_safety::same_step(before.nodes@[i as int]);
    handler_safety::step_identity(before.nodes@[i as int], before.nodes@[i as int]);
    assert(before.nodes@ =~= before.nodes@.update(i as int, before.nodes@[i as int]));
}

}
