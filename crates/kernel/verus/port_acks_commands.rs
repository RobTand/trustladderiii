//! #806: exact owner-command frames and complete real-apply safety.
use super::port_acks_specs::*;
use super::retirement::{accepted, bounded, domain, holders, node_rules};
use super::*;

verus! {
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

/// State-table commands / #806: preserve every node checker rule and Q54 assertion.
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
    reveal(command_node); reveal(retirement::stopped); reveal(node_rules);
    reveal(node_transition); reveal(q54);
    match c {
        Command::Stop => {}, Command::ChangeRequest => {},
        Command::Pause => {}, Command::Resume => {}, _ => {},
    }
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
        command_post(w, result.0, 0, id, c, port_acks_specs::empty(), result.1),
        result.1.len == 0,
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
    apply(&mut w, Event::Command { node: NodeId(0), id, command: c }, &mut out);
    kernel_proof! {
        command_world(original@, w, id, c, port_acks_specs::empty(), out);
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out)
}
}
