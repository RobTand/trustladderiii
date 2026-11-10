//! E11.1 (Stop, CONTRACTS.md v2.7) owner definitions: the opaque framing predicates (form rule 2), the
//! forwarding atom and their intro lemmas, in their own module (kernel master 11-10: module context).
use super::*;

verus! {

/// E11.1 v2.7 framing for a fresh Stop of a live item (form rule 2): counters, credit, generation.
#[verifier::opaque]
pub(super) open spec fn stop_live_frame(b: Node, n: Node) -> bool {
    n.run_failures == b.run_failures && n.run_incidents == b.run_incidents
        && n.fx_failures == b.fx_failures && n.fx_incidents == b.fx_incidents
        && n.credit == b.credit && n.generation as int == b.generation as int + 1
}

/// E11.1b framing for a fresh Stop of an item already stopping (form rule 2): teardown state kept.
#[verifier::opaque]
pub(super) open spec fn stop_stopping_frame(b: Node, n: Node) -> bool {
    n.phase == b.phase && n.pos == b.pos && n.holds == b.holds && n.pool == b.pool
        && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
        && n.effect == b.effect && n.next_seq == b.next_seq && n.question == b.question
}

/// E11.1 intro (rule 6): R24's rows (direct) and both framing predicates over the literal stopped node.
pub(super) proof fn stop_e11_frames(b: Node, id: u64)
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
            })
            && stop_live_frame(b, n))
        && (b.phase is Stopping ==> stop_stopping_frame(b, n)) }),
{
    reveal(retirement::stopped);
    reveal(stop_live_frame);
    reveal(stop_stopping_frame);
}

/// E11.1 forwarding atom (form rule 2): R24's rows and both framing predicates, bundled for on_command.
#[verifier::opaque]
pub(super) open spec fn stop_post(b: Node, n: Node, id: u64) -> bool {
    (!(b.phase is Closed || b.phase is Stopping)
            ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 0
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool
            && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            })
            && stop_live_frame(b, n))
        && (b.phase is Stopping ==> stop_stopping_frame(b, n))
}

/// E11.1: intro for the forwarding atom.
pub(super) proof fn stop_post_intro(b: Node, n: Node, id: u64)
    requires (!(b.phase is Closed || b.phase is Stopping)
            ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 0
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool
            && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            })
            && stop_live_frame(b, n))
        && (b.phase is Stopping ==> stop_stopping_frame(b, n)),
    ensures stop_post(b, n, id),
{
    reveal(stop_post);
}


/// E2.N4a (CONTRACTS.md v2.20, R28 framing, form rule 2): a waiting owner stays a waiting owner.
#[verifier::opaque]
pub(super) open spec fn owner_waiting_kept(b: Node, n: Node) -> bool {
    dependencies::owner_waiting_ok(b) ==> dependencies::owner_waiting_ok(n)
}

/// E2.N4b (CONTRACTS.md v2.20, R28 framing, form rule 2): the dependency endpoint is unchanged.
#[verifier::opaque]
pub(super) open spec fn endpoint_kept(b: Node, n: Node) -> bool {
    dependencies::same_endpoint(b, n)
}

/// E2.N4a intro over the stop bridges' written-node term (Stop: Stopped, ChangeRequest: Replaced).
pub(super) proof fn stop_owner_waiting_frame(b: Node, id: u64, o: Outcome)
    ensures owner_waiting_kept(b, retirement::stopped(Node { last_cmd: id, ..b }, o)),
{
    reveal(owner_waiting_kept);
    reveal(retirement::stopped);
}

/// E2.N4b intro over pause_world's written-node term.
pub(super) proof fn pause_endpoint_frame(b: Node, id: u64)
    ensures endpoint_kept(b, apply_command_safety::pause_node(b, id)),
{
    reveal(endpoint_kept);
}

/// E2.N4a / E2.N4b on on_command's repeated-command path (the node is unchanged).
pub(super) proof fn n4_frames_refl(b: Node)
    ensures owner_waiting_kept(b, b), endpoint_kept(b, b),
{
    reveal(owner_waiting_kept);
    reveal(endpoint_kept);
}

}
