//! #806: isolate CI-failure retry frames and pre-enqueue input equivalence.
use super::*;

verus! {
/// #806 / Q54: verified paid-retry bounds imply the fact frame for an open node.
pub(super) proof fn retry_frame(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(), !(before.nodes@[i as int].phase is Closed),
        retry_bounds(before, after, i as int, out_before.len, out_after.len),
    ensures retirement::fact_frame(before, after, i, out_before, out_after),
{
    reveal(retry_bounds);
    reveal(retirement::fact_frame);
}

/// #806 / Q50: pre-enqueue cancellation does not alter the infrastructure result.
pub(super) proof fn cancellation_input(before: Node, prepared: Node)
    requires before.phase == Phase::Queued || (before.phase == Phase::Publishing
        && current_item(before) == Some(Item::Do(EffectKind::Enqueue))),
        prepared == (Node { effect: match before.effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || before.phase == Phase::Publishing, ..e }),
            None => None,
        }, ..before }),
    ensures retirement::fact_node(prepared, Fact::CiFailed { infra: true })
        == retirement::fact_node(before, Fact::CiFailed { infra: true }),
{
    if before.phase == Phase::Publishing {
        reveal(retirement::fact_node);
        if before.run_failures == 0 {
            assert(retirement::fact_node(prepared, Fact::CiFailed { infra: true })
                == retirement::fact_node(before, Fact::CiFailed { infra: true }));
        } else {
            assert(retirement::fact_node(prepared, Fact::CiFailed { infra: true })
                == retirement::fact_node(before, Fact::CiFailed { infra: true }));
        }
    } else {
        assert(prepared == before);
    }
}
pub(super) proof fn no_change(b: World, a: World, i: usize, out: Outbox)
    requires i < b.nodes@.len(),
        a.nodes@[i as int].phase == b.nodes@[i as int].phase,
        a.nodes@[i as int].repo == b.nodes@[i as int].repo,
        a.nodes@[i as int].footprint == b.nodes@[i as int].footprint,
        a.nodes@[i as int].pool == b.nodes@[i as int].pool,
        a.nodes@[i as int].holds == b.nodes@[i as int].holds,
        a.nodes@[i as int].pos == b.nodes@[i as int].pos,
        a.nodes@[i as int].generation == b.nodes@[i as int].generation,
        a.nodes@[i as int].next_seq == b.nodes@[i as int].next_seq,
        a.nodes@[i as int].credit == b.nodes@[i as int].credit,
        a.nodes@[i as int].last_cmd == b.nodes@[i as int].last_cmd,
        a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j],
        a.next_ticket == b.next_ticket,
        a.next_question == b.next_question,
        a.gates@ == b.gates@,
        a.pool_capacity == b.pool_capacity,
        a.fail_fast == b.fail_fast,
        a.file_sets@ == b.file_sets@,
        a.dependencies@ == b.dependencies@,
    ensures retirement::fact_frame(b, a, i, out, out),
{ reveal(retirement::fact_frame); }

pub(super) proof fn prepared_frame(b: World, p: World, a: World, i: usize, bo: Outbox, ao: Outbox)
    requires i < b.nodes@.len(),
        retirement::fact_frame(p, a, i, bo, ao),
        p.nodes@[i as int].phase == b.nodes@[i as int].phase,
        p.nodes@[i as int].repo == b.nodes@[i as int].repo,
        p.nodes@[i as int].footprint == b.nodes@[i as int].footprint,
        p.nodes@[i as int].pool == b.nodes@[i as int].pool,
        p.nodes@[i as int].holds == b.nodes@[i as int].holds,
        p.nodes@[i as int].pos == b.nodes@[i as int].pos,
        p.nodes@[i as int].generation == b.nodes@[i as int].generation,
        p.nodes@[i as int].next_seq == b.nodes@[i as int].next_seq,
        p.nodes@[i as int].credit == b.nodes@[i as int].credit,
        p.nodes@[i as int].last_cmd == b.nodes@[i as int].last_cmd,
        p.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> p.nodes@[j] == b.nodes@[j],
        p.next_ticket == b.next_ticket,
        p.next_question == b.next_question,
        p.gates@ == b.gates@,
        p.pool_capacity == b.pool_capacity,
        p.fail_fast == b.fail_fast,
        p.file_sets@ == b.file_sets@,
        p.dependencies@ == b.dependencies@,
    ensures retirement::fact_frame(b, a, i, bo, ao),
{ reveal(retirement::fact_frame); }

pub(super) proof fn ignored_fields(n: Node, infra: bool)
    requires n.phase != Phase::Queued && !(n.phase == Phase::Publishing
        && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
    ensures retirement::fact_node(n, Fact::CiFailed { infra }).phase == n.phase,
        retirement::fact_node(n, Fact::CiFailed { infra }).pos == n.pos,
        retirement::fact_node(n, Fact::CiFailed { infra }).repo == n.repo,
        retirement::fact_node(n, Fact::CiFailed { infra }).footprint == n.footprint,
        retirement::fact_node(n, Fact::CiFailed { infra }).generation == n.generation,
        retirement::fact_node(n, Fact::CiFailed { infra }).next_seq == n.next_seq,
        retirement::fact_node(n, Fact::CiFailed { infra }).effect == n.effect,
        retirement::fact_node(n, Fact::CiFailed { infra }).run == n.run,
        retirement::fact_node(n, Fact::CiFailed { infra }).job == n.job,
        retirement::fact_node(n, Fact::CiFailed { infra }).pool == n.pool,
        retirement::fact_node(n, Fact::CiFailed { infra }).ticket == n.ticket,
        retirement::fact_node(n, Fact::CiFailed { infra }).holds == n.holds,
        retirement::fact_node(n, Fact::CiFailed { infra }).paused == n.paused,
        retirement::fact_node(n, Fact::CiFailed { infra }).last_cmd == n.last_cmd,
        retirement::fact_node(n, Fact::CiFailed { infra }).question == n.question,
        retirement::fact_node(n, Fact::CiFailed { infra }).attempt == n.attempt,
        retirement::fact_node(n, Fact::CiFailed { infra }).round == n.round,
        retirement::fact_node(n, Fact::CiFailed { infra }).replanned == n.replanned,
        retirement::fact_node(n, Fact::CiFailed { infra }).plan_retries == n.plan_retries,
        retirement::fact_node(n, Fact::CiFailed { infra }).audit_rounds == n.audit_rounds,
        retirement::fact_node(n, Fact::CiFailed { infra }).early_end == n.early_end,
        retirement::fact_node(n, Fact::CiFailed { infra }).run_failures == n.run_failures,
        retirement::fact_node(n, Fact::CiFailed { infra }).run_incidents == n.run_incidents,
        retirement::fact_node(n, Fact::CiFailed { infra }).fx_failures == n.fx_failures,
        retirement::fact_node(n, Fact::CiFailed { infra }).fx_incidents == n.fx_incidents,
        retirement::fact_node(n, Fact::CiFailed { infra }).waiting_incident == n.waiting_incident,
        retirement::fact_node(n, Fact::CiFailed { infra }).incident_needs_rob == n.incident_needs_rob,
        retirement::fact_node(n, Fact::CiFailed { infra }).proposed == n.proposed,
        retirement::fact_node(n, Fact::CiFailed { infra }).adopted == n.adopted,
        retirement::fact_node(n, Fact::CiFailed { infra }).head == n.head,
        retirement::fact_node(n, Fact::CiFailed { infra }).checked == n.checked,
        retirement::fact_node(n, Fact::CiFailed { infra }).approved == n.approved,
        retirement::fact_node(n, Fact::CiFailed { infra }).merged == n.merged,
        retirement::fact_node(n, Fact::CiFailed { infra }).pr_open == n.pr_open,
        retirement::fact_node(n, Fact::CiFailed { infra }).had_pr == n.had_pr,
        retirement::fact_node(n, Fact::CiFailed { infra }).worktree == n.worktree,
        retirement::fact_node(n, Fact::CiFailed { infra }).rechecks == n.rechecks,
        retirement::fact_node(n, Fact::CiFailed { infra }).review_restarted == n.review_restarted,
        retirement::fact_node(n, Fact::CiFailed { infra }).credit == n.credit,
        retirement::fact_node(n, Fact::CiFailed { infra }).priority == n.priority,
        retirement::fact_node(n, Fact::CiFailed { infra }).review_stop == n.review_stop,
        retirement::fact_node(n, Fact::CiFailed { infra }).planning_started == n.planning_started,
        retirement::fact_node(n, Fact::CiFailed { infra }).waiting_prerequisite == n.waiting_prerequisite,
{ reveal(retirement::fact_node); }

}
