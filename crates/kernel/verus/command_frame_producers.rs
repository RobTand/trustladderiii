//! command_frames producers (CONTRACTS.md v2.35, owner apply): value lemmas over each world bridge's own
//! written-node term (rule 6), so the bridges carry R24's rows and the opaque command_frame_rows atom and the
//! entries export them by literal match. Separate module (module context).
use super::*;

verus! {

/// Stop: R24's rows (direct) and command_frame_rows over stopped(Node { last_cmd: id, ..b }, Stopped).
pub(super) proof fn stop_frame_value(b: Node, id: u64)
    requires node_headroom(b),
    ensures ({ let n = retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped);
        (!(b.phase is Closed || b.phase is Stopping) ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 0
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool
            && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            }))
        && command_frames::command_frame_rows(b, n, id, Command::Stop) }),
{
    reveal(retirement::stopped);
    let n = retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped);
    command_frames::command_frame_rows_intro(b, n, id, Command::Stop);
}

/// ChangeRequest: command_frame_rows (N4a) over stopped(Node { last_cmd: id, ..b }, Replaced).
pub(super) proof fn cr_frame_value(b: Node, id: u64)
    ensures command_frames::command_frame_rows(b,
        retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Replaced), id, Command::ChangeRequest),
{
    reveal(retirement::stopped);
    command_frames::command_frame_rows_intro(b,
        retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Replaced), id, Command::ChangeRequest);
}

/// Pause: command_frame_rows (N4b) over pause_world's pause_node(b, id).
pub(super) proof fn pause_frame_value(b: Node, id: u64)
    ensures command_frames::command_frame_rows(b, apply_command_safety::pause_node(b, id), id, Command::Pause),
{
    command_frames::command_frame_rows_intro(b, apply_command_safety::pause_node(b, id), id, Command::Pause);
}

}
