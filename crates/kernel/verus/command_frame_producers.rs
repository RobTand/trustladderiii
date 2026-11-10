//! command_frames producers (CONTRACTS.md v2.35, owner apply): value lemmas over each world bridge's own
//! written-node term (rule 6), so the bridges carry the opaque R24 atom (r24_rows) and command_frame_rows and
//! the entries export them by literal match. Separate module (module context).
use super::*;

verus! {

/// Stop, R24 (rule 6): the opaque R24 atom over stopped(Node { last_cmd: id, ..b }, Stopped).
pub(super) proof fn stop_r24_value(b: Node, id: u64)
    ensures r24_rows::stop_rows(b, retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped), id),
{
    reveal(retirement::stopped);
    r24_rows::stop_rows_intro(b, retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped), id);
}

/// Stop, v2.7 framing row (rule 6): a fresh Stop of a live item keeps counters and credit; generation + 1.
pub(super) proof fn stop_live_value(b: Node, id: u64)
    requires node_headroom(b),
    ensures ({ let n = retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped);
        (id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
            ==> n.run_failures == b.run_failures && n.run_incidents == b.run_incidents
                && n.fx_failures == b.fx_failures && n.fx_incidents == b.fx_incidents
                && n.credit == b.credit && n.generation as int == b.generation as int + 1 }),
{
    reveal(retirement::stopped);
}

/// Stop, E11.1b row (rule 6): a fresh Stop of an item already stopping keeps its teardown state.
pub(super) proof fn stop_stopping_value(b: Node, id: u64)
    ensures ({ let n = retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped);
        (id > b.last_cmd && b.phase is Stopping)
            ==> n.phase == b.phase && n.pos == b.pos && n.holds == b.holds
                && n.pool == b.pool && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open
                && n.worktree == b.worktree && n.effect == b.effect
                && n.next_seq == b.next_seq && n.question == b.question }),
{
    reveal(retirement::stopped);
}

/// Stop, E2.N4a row (rule 6): a waiting owner stays a waiting owner.
pub(super) proof fn stop_owner_value(b: Node, id: u64)
    ensures dependencies::owner_waiting_ok(b) ==> dependencies::owner_waiting_ok(retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped)),
{
    reveal(retirement::stopped);
}

/// Stop, E2 framing: command_frame_rows over stopped(Node { last_cmd: id, ..b }, Stopped), from the three
/// row lemmas (no reveal here).
pub(super) proof fn stop_frame_value(b: Node, id: u64)
    requires node_headroom(b),
    ensures command_frames::command_frame_rows(b, retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped), id, Command::Stop),
{
    stop_live_value(b, id);
    stop_stopping_value(b, id);
    stop_owner_value(b, id);
    command_frames::command_frame_rows_intro(b, retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped), id, Command::Stop);
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
