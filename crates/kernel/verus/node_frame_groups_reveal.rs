use super::*;
use vstd::prelude::*;

verus! {
pub proof fn ops_same_reveal(b: Node, a: Node)
    ensures node_frame_groups::ops_same(b, a) == (a.phase == b.phase
        && a.repo == b.repo
        && a.footprint == b.footprint
        && a.effect == b.effect
        && a.ticket == b.ticket
        && a.holds == b.holds
        && a.paused == b.paused
        && a.question == b.question
        && a.waiting_incident == b.waiting_incident
        && a.pos == b.pos),
{
    reveal(node_frame_groups::ops_same);
}

pub proof fn ledger_same_reveal(b: Node, a: Node)
    ensures node_frame_groups::ledger_same(b, a) == (a.credit == b.credit),
{
    reveal(node_frame_groups::ledger_same);
}

pub proof fn counters_same_reveal(b: Node, a: Node)
    ensures node_frame_groups::counters_same(b, a) == (a.generation == b.generation
        && a.next_seq == b.next_seq
        && a.last_cmd == b.last_cmd
        && a.attempt == b.attempt
        && a.replanned == b.replanned),
{
    reveal(node_frame_groups::counters_same);
}

pub proof fn pr_same_reveal(b: Node, a: Node)
    ensures node_frame_groups::pr_same(b, a) == (a.proposed == b.proposed
        && a.adopted == b.adopted
        && a.review_stop == b.review_stop),
{
    reveal(node_frame_groups::pr_same);
}

pub proof fn frame_intro(b: Node, a: Node)
    requires
        a.phase == b.phase,
        a.repo == b.repo,
        a.footprint == b.footprint,
        a.effect == b.effect,
        a.ticket == b.ticket,
        a.holds == b.holds,
        a.paused == b.paused,
        a.question == b.question,
        a.waiting_incident == b.waiting_incident,
        a.pos == b.pos,
        a.credit == b.credit,
        a.generation == b.generation,
        a.next_seq == b.next_seq,
        a.last_cmd == b.last_cmd,
        a.attempt == b.attempt,
        a.replanned == b.replanned,
        a.proposed == b.proposed,
        a.adopted == b.adopted,
        a.review_stop == b.review_stop,
    ensures
        node_frame_groups::ops_same(b, a),
        node_frame_groups::ledger_same(b, a),
        node_frame_groups::counters_same(b, a),
        node_frame_groups::pr_same(b, a),
{
    ops_same_reveal(b, a);
    ledger_same_reveal(b, a);
    counters_same_reveal(b, a);
    pr_same_reveal(b, a);
}

pub proof fn mark_handled_resources_rows(b: Node, a: Node, kind: EffectKind)
    requires
        node_frame_groups::ops_same(b, a),
        node_frame_groups::ledger_same(b, a),
        node_frame_groups::counters_same(b, a),
        node_frame_groups::pr_same(b, a),
        a.run == if kind == EffectKind::StopRun { None } else { b.run },
        a.pool == if kind == EffectKind::StopRun { false } else { b.pool },
        a.job == if kind == EffectKind::WithdrawJob { None } else { b.job },
        a.pr_open == if kind == EffectKind::ClosePr { false } else { b.pr_open },
        a.worktree == if kind == EffectKind::RemoveWorktree { false } else { b.worktree },
    ensures a.phase == b.phase,
        a.repo == b.repo,
        a.footprint == b.footprint,
        a.generation == b.generation,
        a.next_seq == b.next_seq,
        a.effect == b.effect,
        a.ticket == b.ticket,
        a.holds == b.holds,
        a.paused == b.paused,
        a.last_cmd == b.last_cmd,
        a.question == b.question,
        a.waiting_incident == b.waiting_incident,
        a.credit == b.credit,
        a.pos == b.pos,
        a.run == if kind == EffectKind::StopRun { None } else { b.run },
        a.pool == if kind == EffectKind::StopRun { false } else { b.pool },
        a.job == if kind == EffectKind::WithdrawJob { None } else { b.job },
        a.pr_open == if kind == EffectKind::ClosePr { false } else { b.pr_open },
        a.worktree == if kind == EffectKind::RemoveWorktree { false } else { b.worktree },
        a.proposed == b.proposed, a.adopted == b.adopted,
        a.review_stop == b.review_stop, a.attempt == b.attempt,
        a.replanned == b.replanned,
{
    ops_same_reveal(b, a);
    ledger_same_reveal(b, a);
    counters_same_reveal(b, a);
    pr_same_reveal(b, a);
}
}
