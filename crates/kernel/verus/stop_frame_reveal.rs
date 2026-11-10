//! E11.1 consumer reveal (form rule 2): the per-field rows behind apply_command_safety's Stop framing
//! predicates. A separate module, so the reveal stays out of the owner's and the entries' queries.
use super::*;

verus! {

pub(super) proof fn stop_frames_reveal(b: Node, n: Node)
    ensures apply_command_safety::stop_live_frame(b, n) ==> (n.run_failures == b.run_failures && n.run_incidents == b.run_incidents
        && n.fx_failures == b.fx_failures && n.fx_incidents == b.fx_incidents
        && n.credit == b.credit && n.generation as int == b.generation as int + 1),
        apply_command_safety::stop_stopping_frame(b, n) ==> (n.phase == b.phase && n.pos == b.pos && n.holds == b.holds && n.pool == b.pool
        && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
        && n.effect == b.effect && n.next_seq == b.next_seq && n.question == b.question),
{
    reveal(apply_command_safety::stop_live_frame);
    reveal(apply_command_safety::stop_stopping_frame);
}

}
