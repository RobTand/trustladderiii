//! K5 E2 framing and E1.5 (CONTRACTS.md v2.35, owner apply): opaque command framing rows on `on_command`
//! and the opaque Command-row post on `events::apply` (form rule 2). Frozen contract-first by the integration
//! architect; apply implements its producers to exactly these names. Consumers reveal them only through
//! `command_frames_reveal` (a separate module).
use super::*;

verus! {

/// E2 framing (rule 2): R24's Stop rows (v2.45), N4a, N4b, E11.1b and the v2.7 Stop counter/credit/generation rows, verbatim as frozen,
/// over the input node `b` and the literal written node `n`. R24's rows stay direct on `on_command`.
#[verifier::opaque]
pub(super) open spec fn command_frame_rows(b: Node, n: Node, id: u64, command: Command) -> bool {
    // E11.1 REQ R24 (v2.45, rule 1: hash-frozen body; carrier command_frames_reveal::r24_stop).
    ((command is Stop && id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
        ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 0
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            }))
    // E11.1 framing (v2.7): a fresh Stop of a live item keeps the counters and credit; generation + 1 exactly.
    && ((command is Stop && id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
        ==> n.run_failures == b.run_failures && n.run_incidents == b.run_incidents
            && n.fx_failures == b.fx_failures && n.fx_incidents == b.fx_incidents
            && n.credit == b.credit && n.generation as int == b.generation as int + 1)
    // E11.1b: a fresh Stop of an item already stopping keeps its teardown state.
    && ((command is Stop && id > b.last_cmd && b.phase is Stopping)
        ==> n.phase == b.phase && n.pos == b.pos && n.holds == b.holds
            && n.pool == b.pool && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open
            && n.worktree == b.worktree && n.effect == b.effect
            && n.next_seq == b.next_seq && n.question == b.question)
    // E2.N4a: Stop and ChangeRequest keep a waiting owner's rows.
    && (((command is Stop || command is ChangeRequest) && dependencies::owner_waiting_ok(b))
        ==> dependencies::owner_waiting_ok(n))
    // E2.N4b: Pause keeps the dependency endpoint.
    && ((command is Pause) ==> dependencies::same_endpoint(b, n))
}

/// Intro for `command_frame_rows` (apply's producers end with this call).
pub(super) proof fn command_frame_rows_intro(b: Node, n: Node, id: u64, command: Command)
    requires
        (command is Stop && id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
        ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 0
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            }),
        (command is Stop && id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
            ==> n.run_failures == b.run_failures && n.run_incidents == b.run_incidents
                && n.fx_failures == b.fx_failures && n.fx_incidents == b.fx_incidents
                && n.credit == b.credit && n.generation as int == b.generation as int + 1,
        (command is Stop && id > b.last_cmd && b.phase is Stopping)
            ==> n.phase == b.phase && n.pos == b.pos && n.holds == b.holds
                && n.pool == b.pool && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open
                && n.worktree == b.worktree && n.effect == b.effect
                && n.next_seq == b.next_seq && n.question == b.question,
        ((command is Stop || command is ChangeRequest) && dependencies::owner_waiting_ok(b))
            ==> dependencies::owner_waiting_ok(n),
        (command is Pause) ==> dependencies::same_endpoint(b, n),
    ensures command_frame_rows(b, n, id, command),
{
    reveal(command_frame_rows);
}

/// E1.5 (rule 2): `events::apply`'s Command rows. v2.40 (lead's premise sweep): the trusted rows that apply_command
/// does not prove unconditionally carry the kernel's guarantee as premise: `node_rules(before, b)` for
/// `command_frame_rows`, `checking::accepted(before)` for the out-of-range frame. apply_command's N-cut rows, the command-step/answer-pair row,
/// the E2 framing rows (`command_frame_rows`), v2.18's Answer incident row, and the out-of-range frame over
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
                && (retirement::node_rules(before, b) ==> command_frame_rows(b, n, id, command))
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
                && (retirement::node_rules(before, b) ==> command_frame_rows(b, n, id, command))
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
