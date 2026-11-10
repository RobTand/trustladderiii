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

}
