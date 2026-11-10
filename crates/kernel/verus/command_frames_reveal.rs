//! K5 consumer reveals (CONTRACTS.md v2.35, form rule 2): the only place `command_frames`' opaque predicates are
//! opened for consumers. Each ensures is one frozen row, verbatim (rule 7).
use super::*;

verus! {

/// Reveal of `command_frames::command_frame_rows` (v2.56): its two parts. Rows then come from
/// `stop_frame_reveal::stop_post_rows` (R24) / `stop_frame_reveal::stop_frames_reveal` and `command_owner_rows_reveal`.
pub(super) proof fn command_frame_rows_reveal(b: Node, n: Node, id: u64, command: Command)
    requires command_frames::command_frame_rows(b, n, id, command),
    ensures
        (command is Stop && id > b.last_cmd) ==> stop_frames::stop_post(b, n, id),
        command_frames::command_owner_rows(b, n, command),
{
    reveal(command_frames::command_frame_rows);
}

/// Reveal of `command_frames::command_owner_rows`: N4a and N4b, one ensures each, verbatim.
pub(super) proof fn command_owner_rows_reveal(b: Node, n: Node, command: Command)
    requires command_frames::command_owner_rows(b, n, command),
    ensures
        ((command is Stop || command is ChangeRequest) && dependencies::owner_waiting_ok(b))
            ==> dependencies::owner_waiting_ok(n),
        (command is Pause) ==> dependencies::same_endpoint(b, n),
{
    reveal(command_frames::command_owner_rows);
}

/// Projection of `command_frames::command_rows` for one Command event: every row as its own ensures.
pub(super) proof fn command_rows_project(before: World, after: World, node: NodeId, id: u64, command: Command)
    requires command_frames::command_rows(before, after, Event::Command { node, id, command }),
    ensures
        after.nodes@.len() == before.nodes@.len(),
        after.dependencies@ == before.dependencies@,
        before.next_question <= after.next_question,
        before.nodes@.len() <= u64::MAX,
        node.0 < before.nodes@.len() ==> (retirement::node_rules(before, before.nodes@[node.0 as int])
            ==> retirement::node_rules(after, after.nodes@[node.0 as int])),
        node.0 < before.nodes@.len() ==> (after.nodes@[node.0 as int].holds ==> before.nodes@[node.0 as int].holds),
        node.0 < before.nodes@.len() ==> (after.nodes@[node.0 as int].pool ==> before.nodes@[node.0 as int].pool),
        node.0 < before.nodes@.len() ==> (after.nodes@[node.0 as int].ticket == before.nodes@[node.0 as int].ticket
            || after.nodes@[node.0 as int].ticket.is_none()),
        node.0 < before.nodes@.len() ==> after.nodes@[node.0 as int].repo == before.nodes@[node.0 as int].repo,
        node.0 < before.nodes@.len() ==> after.nodes@[node.0 as int].footprint == before.nodes@[node.0 as int].footprint,
        node.0 < before.nodes@.len() ==> after.nodes@ == before.nodes@.update(node.0 as int, after.nodes@[node.0 as int]),
        node.0 < before.nodes@.len() ==> (match command {
            Command::Answer { question, answer } => handler_safety::answer_pair(
                before.nodes@[node.0 as int], after.nodes@[node.0 as int],
                id > before.nodes@[node.0 as int].last_cmd, question, answer, after.dependencies@, node),
            _ => handler_safety::command_step(before.nodes@[node.0 as int], after.nodes@[node.0 as int]),
        }),
        node.0 < before.nodes@.len() ==> (retirement::node_rules(before, before.nodes@[node.0 as int])
            ==> command_frames::command_frame_rows(before.nodes@[node.0 as int], after.nodes@[node.0 as int], id, command)),
        node.0 < before.nodes@.len() ==> (match command {
            Command::Answer { .. } => retirement::node_rules(before, before.nodes@[node.0 as int])
                ==> (after.nodes@[node.0 as int].incident_needs_rob ==> after.nodes@[node.0 as int].waiting_incident),
            _ => true,
        }),
        checking::accepted(before) && node.0 >= before.nodes@.len() ==> after.nodes@ == before.nodes@,
        checking::accepted(before) && node.0 >= before.nodes@.len() ==> after.next_question == before.next_question,
        checking::accepted(before) && node.0 >= before.nodes@.len() ==> after.file_sets@ == before.file_sets@,
{
    reveal(command_frames::command_rows);
}

}
