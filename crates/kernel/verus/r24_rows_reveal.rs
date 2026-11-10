//! E11.1 REQ R24, named lemma (form rule 1): the only place `r24_rows::stop_rows` is opened for consumers.
use super::*;

verus! {

/// R24 exactly as frozen (CONTRACTS.md v2.7), over a Stop's input node `b` and written node `n`.
pub(super) proof fn r24_stop(b: Node, n: Node, id: u64)
    requires r24_rows::stop_rows(b, n, id),
    ensures (id > b.last_cmd && !(b.phase is Closed || b.phase is Stopping))
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
{
    reveal(r24_rows::stop_rows);
}

}
