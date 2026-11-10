//! E11.1 REQ R24 carrier (kernel master 11-10; form rules 1 and 2, owner apply): R24's rows behind one opaque
//! atom, so no entry links old/final World fields for them. Consumers get the rows only through the named
//! lemma `r24_rows_reveal::r24_stop`, whose ensures is R24 exactly. Separate module (module context).
use super::*;

verus! {

/// R24 (opaque): a fresh Stop of a live item stops it, keeping its holds, pool, run, job, PR, worktree and
/// effect identity, over the input node `b` and the literal written node `n`.
#[verifier::opaque]
pub(super) open spec fn stop_rows(b: Node, n: Node, id: u64) -> bool {
    (id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
        ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 1
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool
            && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            })
}

/// Intro for `stop_rows` (apply's Stop producer and on_command's repeat path end with this call).
pub(super) proof fn stop_rows_intro(b: Node, n: Node, id: u64)
    requires (id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
        ==> n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == 1
            && n.ticket.is_none() && n.question.is_none() && !n.waiting_incident
            && n.holds == b.holds && n.pool == b.pool
            && n.run == b.run && n.job == b.job && n.pr_open == b.pr_open && n.worktree == b.worktree
            && n.next_seq == b.next_seq && n.last_cmd == id
            && (match b.effect {
                None => n.effect.is_none(),
                Some(e) => n.effect.is_some() && n.effect.unwrap().seq == e.seq
                    && n.effect.unwrap().kind == e.kind && n.effect.unwrap().cancelling,
            }),
    ensures stop_rows(b, n, id),
{
    reveal(stop_rows);
}

}
