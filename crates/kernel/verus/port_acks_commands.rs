//! #806: exact owner-command frames and complete real-apply safety.
use super::port_acks_specs::*;
use super::retirement::{accepted, bounded, domain, holders, node_rules};
use super::*;

verus! {
/// State-table duplicate command: the real early return retains every field.
pub(super) proof fn duplicate_result(n: Node, id: u64, c: Command)
    requires id <= n.last_cmd,
    ensures command_result(n, n, id, c),
{ reveal(command_result); reveal(command_node); }



/// #806: frame the real last_cmd assignment independently of the selected command.
pub(super) proof fn id_result(b: Node, a: Node, id: u64)
    requires a.phase == (b.phase),
        a.pos == (b.pos),
        a.repo == (b.repo),
        a.footprint == (b.footprint),
        a.generation == (b.generation),
        a.next_seq == (b.next_seq),
        a.effect == (b.effect),
        a.run == (b.run),
        a.job == (b.job),
        a.pool == (b.pool),
        a.ticket == (b.ticket),
        a.holds == (b.holds),
        a.paused == (b.paused),
        a.last_cmd == (id),
        a.question == (b.question),
        a.attempt == (b.attempt),
        a.round == (b.round),
        a.replanned == (b.replanned),
        a.plan_retries == (b.plan_retries),
        a.audit_rounds == (b.audit_rounds),
        a.early_end == (b.early_end),
        a.run_failures == (b.run_failures),
        a.run_incidents == (b.run_incidents),
        a.fx_failures == (b.fx_failures),
        a.fx_incidents == (b.fx_incidents),
        a.waiting_incident == (b.waiting_incident),
        a.incident_needs_rob == (b.incident_needs_rob),
        a.proposed == (b.proposed),
        a.adopted == (b.adopted),
        a.head == (b.head),
        a.checked == (b.checked),
        a.approved == (b.approved),
        a.merged == (b.merged),
        a.pr_open == (b.pr_open),
        a.had_pr == (b.had_pr),
        a.worktree == (b.worktree),
        a.rechecks == (b.rechecks),
        a.review_restarted == (b.review_restarted),
        a.credit == (b.credit),
        a.priority == (b.priority),
        a.review_stop == (b.review_stop),
        a.planning_started == (b.planning_started),
        a.waiting_prerequisite == (b.waiting_prerequisite),
    ensures id_post(b, a, id),
{ reveal(id_post); }

/// State-table owner commands: compose the existing stopping helper's exact frame.
pub(super) proof fn stopping_result(b: Node, fresh: Node, a: Node, id: u64, c: Command, o: Outcome)
    requires id > b.last_cmd,
        (c == Command::Stop && o == Outcome::Stopped) || (c == Command::ChangeRequest && o == Outcome::Replaced),
        id_post(b, fresh, id), a == retirement::stopped(fresh, o),
    ensures command_result(b, a, id, c),
{ reveal(id_post); reveal(command_result); reveal(command_node); }



/// #805 / #806: compose owner-command progress without expanding rank in the executable handler.
pub(super) proof fn command_progress_result(w: World, b: Node, a: Node, id: u64, c: Command)
    requires owner_command(c), command_result(b, a, id, c),
    ensures node_checked(w, b) && id > b.last_cmd && command_progress_input(b, c)
        ==> distance(a) < distance(b),
{
    reveal(command_result); reveal(command_node);
    if node_checked(w, b) && id > b.last_cmd && command_progress_input(b, c) {
        if c == Command::Resume {
            resume_decreases(w, b);
            cleanup_lemmas::resource_distance(Node { paused: false, ..b }, a);
        } else if c == Command::ChangeRequest {
            let fresh = Node { last_cmd: id, ..b };
            allocation_stopping_lemmas::stopping_progress(fresh, a, Outcome::Replaced);
            cleanup_lemmas::resource_distance(b, fresh);
        }
    }
}



/// #806: the existing production Resume equation is the same exact owner-command update.
pub(super) proof fn resume_result(b: Node, a: Node, id: u64)
    requires progress_gate_apply::resume_post(b, a, id, Command::Resume),
    ensures command_result(b, a, id, Command::Resume),
{
    reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed);
    reveal(command_result); reveal(command_node);
}

/// #806: the direct Pause frame is the exact owner-command node equation.
pub(super) proof fn pause_to_command_result(b: Node, a: Node, id: u64)
    requires pause_post(b, a, id, Command::Pause),
    ensures command_result(b, a, id, Command::Pause),
{ reveal(pause_post); reveal(paused_node); reveal(command_result); reveal(command_node); }

/// #806: compose the real command's node, immutable world fields and exact output frame.
pub(super) proof fn finish_command(b: World, a: World, i: usize, id: u64, c: Command, ob: Outbox, oa: Outbox)
    requires i < b.nodes@.len(), owner_command(c), a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j],
        a.next_ticket == b.next_ticket, a.gates@ == b.gates@,
        a.file_sets@ == b.file_sets@, a.dependencies@ == b.dependencies@,
        a.pool_capacity == b.pool_capacity, a.fail_fast == b.fail_fast,
        command_result(b.nodes@[i as int], a.nodes@[i as int], id, c),
        command_io(b.next_question, a.next_question, ob, oa, c),
    ensures command_post(b, a, i, id, c, ob, oa),
{
    reveal(command_post); reveal(command_result); reveal(command_io); reveal(world_frame);
}

/// State-table Stop: only last_cmd and the existing stopping update change.
pub(super) proof fn stop_frame(n: Node, id: u64)
    ensures command_node(n, id, Command::Stop) == if id <= n.last_cmd { n }
        else { retirement::stopped(Node { last_cmd: id, ..n }, Outcome::Stopped) },
{ reveal(command_node); }

/// State-table ChangeRequest: preserve an existing teardown and Closed outcome.
pub(super) proof fn change_request_frame(n: Node, id: u64)
    ensures command_node(n, id, Command::ChangeRequest) == if id <= n.last_cmd { n }
        else { retirement::stopped(Node { last_cmd: id, ..n }, Outcome::Replaced) },
{ reveal(command_node); }

/// State-table Pause: no resources or counters change; Stopping/Closed ignore pause.
pub(super) proof fn pause_frame(n: Node, id: u64)
    ensures command_node(n, id, Command::Pause) == if id <= n.last_cmd { n }
        else { Node { last_cmd: id, paused: n.paused || !(n.phase is Stopping || n.phase is Closed), ..n } },
{ reveal(command_node); }

/// State-table Resume: clear only pause and assign the fresh command ID.
pub(super) proof fn resume_frame(n: Node, id: u64)
    ensures command_node(n, id, Command::Resume) == if id <= n.last_cmd { n }
        else { Node { last_cmd: id, paused: false, ..n } },
{ reveal(command_node); }

/// State-table Stop/ChangeRequest: every complete checker rule survives stopping.
pub(super) proof fn stopping_rules(w: World, n: Node, outcome: Outcome)
    requires node_rules(w, n), node_headroom(n), matches!(outcome, Outcome::Stopped | Outcome::Replaced),
    ensures node_rules(w, retirement::stopped(n, outcome)),
{
    reveal(retirement::stopped); reveal(node_rules);
    match n.phase {
        Phase::Closed(_) => {}, Phase::Stopping(_) => {},
        Phase::Ingesting => {}, Phase::Auditing => {}, Phase::Unplanned => {},
        Phase::Planning { .. } => {}, Phase::Ready(_) => {}, Phase::Making => {},
        Phase::Checking => {}, Phase::Reviewing => {}, Phase::Publishing => {},
        Phase::Queued => {}, Phase::Rebasing => {}, Phase::AwaitingRob => {},
    }
}

/// State-table Pause/Resume: pause and last command ID are absent from checker rules.
pub(super) proof fn pause_id_rules(w: World, n: Node, id: u64, paused: bool)
    ensures node_rules(w, Node { last_cmd: id, paused, ..n }) == node_rules(w, n),
{ reveal(node_rules); }

/// State-table commands / #806: complete checker preservation from separate frames.
pub(super) proof fn command_node_rules(w: World, n: Node, id: u64, c: Command)
    requires node_rules(w, n), node_headroom(n), owner_command(c),
    ensures node_rules(w, command_node(n, id, c)),
{
    reveal(command_node);
    if id > n.last_cmd {
        let fresh = Node { last_cmd: id, ..n };
        pause_id_rules(w, n, id, n.paused);
        match c {
            Command::Stop => stopping_rules(w, fresh, Outcome::Stopped),
            Command::ChangeRequest => stopping_rules(w, fresh, Outcome::Replaced),
            Command::Pause => pause_id_rules(w, n, id, true),
            Command::Resume => pause_id_rules(w, n, id, false), _ => {},
        }
    }
}

/// State-table commands / #806: all shared node and Q54 assertions from exact updates.
pub(super) proof fn command_transition_fields(w: World, n: Node, id: u64, c: Command)
    requires owner_command(c),
    ensures node_transition(n, command_node(n, id, c)),
        q54(n, command_node(n, id, c), w.fail_fast, false),
        command_node(n, id, c).credit == n.credit,
        command_node(n, id, c).holds == n.holds,
        command_node(n, id, c).pool == n.pool,
        command_node(n, id, c).ticket.is_none() || command_node(n, id, c).ticket == n.ticket,
        command_node(n, id, c).repo == n.repo,
        command_node(n, id, c).footprint == n.footprint,
{
    reveal(command_node); reveal(retirement::stopped);
    reveal(node_transition); reveal(q54);
    match c {
        Command::Stop => {}, Command::ChangeRequest => {},
        Command::Pause => {}, Command::Resume => {}, _ => {},
    }
}

/// State-table commands / #806: compose checker safety with the small transition-field lemma.
pub(super) proof fn command_rules(w: World, n: Node, id: u64, c: Command)
    requires node_rules(w, n), node_headroom(n), owner_command(c),
    ensures node_rules(w, command_node(n, id, c)),
        node_transition(n, command_node(n, id, c)),
        q54(n, command_node(n, id, c), w.fail_fast, false),
        command_node(n, id, c).credit == n.credit,
        command_node(n, id, c).holds == n.holds,
        command_node(n, id, c).pool == n.pool,
        command_node(n, id, c).ticket.is_none() || command_node(n, id, c).ticket == n.ticket,
        command_node(n, id, c).repo == n.repo,
        command_node(n, id, c).footprint == n.footprint,
{
    command_node_rules(w, n, id, c);
    command_transition_fields(w, n, id, c);
}

/// State-table global consistency / #806: compose a command's exact node frame.
pub(super) proof fn command_world(b: World, a: World, id: u64, c: Command, ob: Outbox, oa: Outbox)
    requires domain(b), accepted(b), bounded(b), owner_command(c), command_post(b, a, 0, id, c, ob, oa),
    ensures domain(a), accepted(a),
        forall|j: int| 0 <= j < 2 ==> node_transition(b.nodes@[j], a.nodes@[j])
            && q54(b.nodes@[j], a.nodes@[j], b.fail_fast, false),
{
    reveal(command_post); reveal(world_frame); reveal(accepted);
    command_rules(b, b.nodes@[0], id, c);
    retirement::node_frame(b, a, a.nodes@[0]);
    retirement::node_frame(b, a, a.nodes@[1]);
    reveal(node_transition); reveal(q54);
    assert forall|j: int| 0 <= j < 2 implies node_transition(b.nodes@[j], a.nodes@[j])
        && q54(b.nodes@[j], a.nodes@[j], b.fail_fast, false) by { assert(j == 0 || j == 1); }
    assert forall|r: int| 0 <= r < 2 implies holders(a, RepoId(r as u64), 2) <= a.gates@[r].w by {
        assert(holders(a, RepoId(r as u64), 2) == holders(b, RepoId(r as u64), 2));
    }
    footprint::storage_frame(b, a, b.nodes@[0].footprint, b.nodes@[1].footprint);
}
}

kernel_fn! {
/// State-table owner commands / #806: one real apply, complete real checker,
/// every shared node/output and Q54 assertion. No settledness or small-ID premise.
pub(super) fn verify_command_apply(mut w: World, id: u64, c: Command) -> (result: (World, Outbox)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w), owner_command(c),
    ensures domain(result.0), accepted(result.0),
        world_frame(w, result.0, 0), result.0.next_question == w.next_question,
        result.0.nodes@[0] == command_node(w.nodes@[0], id, c),
        empty_outbox(result.1), result.1.len == 0,
        forall|j: int| 0 <= j < 2 ==> node_transition(w.nodes@[j], result.0.nodes@[j])
            && q54(w.nodes@[j], result.0.nodes@[j], w.fail_fast, false),
] proof [reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(bounded);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(accepted); reveal(command_post); reveal(port_acks_specs::empty);] {
    kernel_snapshot! { original = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        dependencies::indexable_definition(w);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    kernel_proof! { world_checked_definition(w); reveal(storage_ordered); }
    let mut out = Outbox::new();
    kernel_snapshot! { original_out = out }
    apply(&mut w, Event::Command { node: NodeId(0), id, command: c }, &mut out);
    kernel_proof! {
        command_world(original@, w, id, c, original_out@, out);
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out)
}
}

verus! {
/// #805 / #806: assigning an Answer command ID preserves every progress-checker field.
pub(super) proof fn answer_command_frame(before: World, after: World, n: Node, id: u64)
    requires after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
    ensures node_checked(before, n) == node_checked(after, Node { last_cmd: id, ..n }),
{
    reveal(node_checked);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}
}

verus! {
/// #805 / #806: compose the existing Resume frame and metric outside the dispatcher.
pub(super) proof fn resume_progress_result(w: World, before: Node, after: Node, id: u64)
    requires progress_gate_apply::resume_post(before, after, id, Command::Resume),
    ensures node_checked(w, before) && id > before.last_cmd
        && command_progress_input(before, Command::Resume) ==> distance(after) < distance(before),
{
    reveal(progress_gate_apply::resume_post); reveal(progress_gate_apply::resumed);
    if node_checked(w, before) && id > before.last_cmd
        && command_progress_input(before, Command::Resume) {
        resume_decreases(w, before);
        cleanup_lemmas::resource_distance(Node { paused: false, ..before }, after);
    }
}
}
