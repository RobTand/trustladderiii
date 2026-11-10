//! K5 E2 framing and E1.5 (CONTRACTS.md v2.35, owner apply): opaque command framing rows on `on_command`
//! and the opaque Command-row post on `events::apply` (form rule 2). Frozen contract-first by the integration
//! architect; apply implements its producers to exactly these names. Consumers reveal them only through
//! `command_frames_reveal` (a separate module).
use super::*;

verus! {

/// E1.5 (rule 2): `events::apply`'s Command rows. v2.61: the E2 framing rows are apply's proven atoms
/// `stop_frames::{stop_post, owner_waiting_kept, endpoint_kept}` (as on `on_command`). v2.40 (lead's premise sweep): the trusted rows that apply_command
/// does not prove unconditionally carry the kernel's guarantee as premise:
/// `checking::accepted(before)` for the out-of-range frame. apply_command's N-cut rows, the command-step/answer-pair row,
/// the E2 framing rows (apply's atoms, v2.61), v2.18's Answer incident row, and the out-of-range frame over
/// every field `checking::accepted` reads that `events::apply` does not already keep (v2.35). N1 covers every
/// command, Answer included (v2.38: apply 004052aa proves it). E2.N4c (restated, v2.37) is NOT here until apply proves it.
#[verifier::opaque]
pub(super) open spec fn command_rows(before: World, after: World, event: Event) -> bool {
    match event {
        Event::Command { node, id, command } =>
            after.nodes@.len() == before.nodes@.len()
            && after.dependencies@ == before.dependencies@
            && before.next_question <= after.next_question
            && before.nodes@.len() <= u64::MAX
            && (node.0 < before.nodes@.len() ==> {
                let b = before.nodes@[node.0 as int];
                let n = after.nodes@[node.0 as int];
                (retirement::node_rules(before, b) ==> retirement::node_rules(after, n))
                && (n.holds ==> b.holds) && (n.pool ==> b.pool)
                && (n.ticket == b.ticket || n.ticket.is_none())
                && n.repo == b.repo && n.footprint == b.footprint
                && after.nodes@ == before.nodes@.update(node.0 as int, n)
                && (match command {
                    Command::Answer { question, answer } => handler_safety::answer_pair(
                        b, n, id > b.last_cmd, question, answer, after.dependencies@, node),
                    _ => handler_safety::command_step(b, n),
                })
                && ((command is Stop && id > b.last_cmd) ==> stop_frames::stop_post(b, n, id))
                && ((command is Stop || command is ChangeRequest) ==> stop_frames::owner_waiting_kept(b, n))
                && ((command is Pause) ==> stop_frames::endpoint_kept(b, n))
                && (match command {
                    Command::Answer { .. } => retirement::node_rules(before, b)
                        ==> (n.incident_needs_rob ==> n.waiting_incident),
                    _ => true,
                })
            })
            && (checking::accepted(before) && node.0 >= before.nodes@.len() ==> after.nodes@ == before.nodes@
                && after.next_question == before.next_question
                && after.file_sets@ == before.file_sets@),
        _ => true,
    }
}

/// Intro for `command_rows` (apply's `apply_command` / `apply_proved` end with this call).
pub(super) proof fn command_rows_intro(before: World, after: World, event: Event)
    requires match event {
        Event::Command { node, id, command } =>
            after.nodes@.len() == before.nodes@.len()
            && after.dependencies@ == before.dependencies@
            && before.next_question <= after.next_question
            && before.nodes@.len() <= u64::MAX
            && (node.0 < before.nodes@.len() ==> {
                let b = before.nodes@[node.0 as int];
                let n = after.nodes@[node.0 as int];
                (retirement::node_rules(before, b) ==> retirement::node_rules(after, n))
                && (n.holds ==> b.holds) && (n.pool ==> b.pool)
                && (n.ticket == b.ticket || n.ticket.is_none())
                && n.repo == b.repo && n.footprint == b.footprint
                && after.nodes@ == before.nodes@.update(node.0 as int, n)
                && (match command {
                    Command::Answer { question, answer } => handler_safety::answer_pair(
                        b, n, id > b.last_cmd, question, answer, after.dependencies@, node),
                    _ => handler_safety::command_step(b, n),
                })
                && ((command is Stop && id > b.last_cmd) ==> stop_frames::stop_post(b, n, id))
                && ((command is Stop || command is ChangeRequest) ==> stop_frames::owner_waiting_kept(b, n))
                && ((command is Pause) ==> stop_frames::endpoint_kept(b, n))
                && (match command {
                    Command::Answer { .. } => retirement::node_rules(before, b)
                        ==> (n.incident_needs_rob ==> n.waiting_incident),
                    _ => true,
                })
            })
            && (checking::accepted(before) && node.0 >= before.nodes@.len() ==> after.nodes@ == before.nodes@
                && after.next_question == before.next_question
                && after.file_sets@ == before.file_sets@),
        _ => true,
    },
    ensures command_rows(before, after, event),
{
    reveal(command_rows);
}

}
