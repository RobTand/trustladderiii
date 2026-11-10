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

/// Stop, E2 framing (rule 6): command_frame_rows over stopped(Node { last_cmd: id, ..b }, Stopped).
pub(super) proof fn stop_frame_value(b: Node, id: u64)
    requires node_headroom(b),
    ensures command_frames::command_frame_rows(b,
        retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped), id, Command::Stop),
{
    reveal(retirement::stopped);
    command_frames::command_frame_rows_intro(b,
        retirement::stopped(Node { last_cmd: id, ..b }, Outcome::Stopped), id, Command::Stop);
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
