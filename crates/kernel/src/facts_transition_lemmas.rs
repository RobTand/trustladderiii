//! #806: expose verified fact-handler frames without unfolding them in every arm.
use super::*;

verus! {
/// #806 fact rows: the existing handler frame implies the original caller clauses.
pub(super) proof fn frame_consequences(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(),
        retirement::fact_frame(before, after, i, out_before, out_after),
    ensures
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        (settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1,
        after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq,
        after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start,
        after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned,
        after.nodes@[i as int].credit.added == before.nodes@[i as int].credit.added,
        after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1,
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        out_after.len <= out_before.len + 2,
        after.nodes@[i as int].last_cmd == before.nodes@[i as int].last_cmd,
{
    reveal(retirement::fact_frame);
}
/// Existing fact-transition state clauses, bundled without strengthening them.
#[verifier::opaque]
pub(super) closed spec fn post(b: World, a: World, i: usize, fact: Fact,
    bo: Outbox, ao: Outbox) -> bool {
    ((fact == Fact::IncidentCleared { by_rob: true }
            && b.nodes@[i as int].waiting_incident && b.nodes@[i as int].effect.is_none()
            && (!b.nodes@[i as int].paused || b.nodes@[i as int].phase is Stopping)
            && !(b.nodes@[i as int].phase is Closed)
            ==> distance(a.nodes@[i as int]) < distance(b.nodes@[i as int])))
        && ((b.nodes@[i as int].phase is Closed ==> a.nodes@[i as int].phase is Closed))
        && (a.nodes@[i as int].repo == b.nodes@[i as int].repo)
        && (a.nodes@[i as int].footprint == b.nodes@[i as int].footprint)
        && ((settle_shape(b.nodes@[i as int]) ==> settle_shape(a.nodes@[i as int])))
        && (a.nodes@.len() == b.nodes@.len())
        && (forall|j: int| 0 <= j < b.nodes@.len() && j != i
            ==> a.nodes@[j] == b.nodes@[j])
        && (a.nodes@[i as int].generation as int <= b.nodes@[i as int].generation as int + 1)
        && (a.nodes@[i as int].next_seq == b.nodes@[i as int].next_seq)
        && (a.nodes@[i as int].credit.start == b.nodes@[i as int].credit.start)
        && (a.nodes@[i as int].credit.earned == b.nodes@[i as int].credit.earned)
        && (a.nodes@[i as int].credit.added == b.nodes@[i as int].credit.added)
        && (a.nodes@[i as int].credit.spent as int <= b.nodes@[i as int].credit.spent as int + 1)
        && (a.next_ticket == b.next_ticket)
        && (a.next_question as int <= b.next_question as int + 1)
        && (a.gates@ == b.gates@)
        && (a.pool_capacity == b.pool_capacity)
        && (a.fail_fast == b.fail_fast)
        && (a.file_sets@ == b.file_sets@)
        && (a.dependencies@ == b.dependencies@)
        && (ao.len <= bo.len + 2)
        && (a.nodes@[i as int].last_cmd == b.nodes@[i as int].last_cmd)
        && (retirement::selected(fact) ==> a.next_question == b.next_question)
        && (retirement::selected(fact) ==> a.dependencies@ == b.dependencies@)
        && (retirement::selected(fact) ==> a.nodes@[i as int]
            == retirement::fact_node(b.nodes@[i as int], fact))
        && (retirement::selected(fact) && retirement::notices_only(bo)
            ==> retirement::notices_only(ao))
        && (fact is Priority ==> ao == bo)
        && (match fact {
            Fact::PrMerged { head, checks_passed } => b.nodes@[i as int].had_pr
                && b.nodes@[i as int].merged.is_none()
                && !(b.nodes@[i as int].phase is Closed)
                && !(b.nodes@[i as int].approved == Some(head) && checks_passed)
                ==> retirement::unapproved_note(ao, i),
            _ => true,
        })
}

pub(super) proof fn pack(b: World, a: World, i: usize, fact: Fact, bo: Outbox, ao: Outbox)
    requires (fact == Fact::IncidentCleared { by_rob: true }
            && b.nodes@[i as int].waiting_incident && b.nodes@[i as int].effect.is_none()
            && (!b.nodes@[i as int].paused || b.nodes@[i as int].phase is Stopping)
            && !(b.nodes@[i as int].phase is Closed)
            ==> distance(a.nodes@[i as int]) < distance(b.nodes@[i as int])),
        (b.nodes@[i as int].phase is Closed ==> a.nodes@[i as int].phase is Closed),
        a.nodes@[i as int].repo == b.nodes@[i as int].repo,
        a.nodes@[i as int].footprint == b.nodes@[i as int].footprint,
        (settle_shape(b.nodes@[i as int]) ==> settle_shape(a.nodes@[i as int])),
        a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i
            ==> a.nodes@[j] == b.nodes@[j],
        a.nodes@[i as int].generation as int <= b.nodes@[i as int].generation as int + 1,
        a.nodes@[i as int].next_seq == b.nodes@[i as int].next_seq,
        a.nodes@[i as int].credit.start == b.nodes@[i as int].credit.start,
        a.nodes@[i as int].credit.earned == b.nodes@[i as int].credit.earned,
        a.nodes@[i as int].credit.added == b.nodes@[i as int].credit.added,
        a.nodes@[i as int].credit.spent as int <= b.nodes@[i as int].credit.spent as int + 1,
        a.next_ticket == b.next_ticket,
        a.next_question as int <= b.next_question as int + 1,
        a.gates@ == b.gates@,
        a.pool_capacity == b.pool_capacity,
        a.fail_fast == b.fail_fast,
        a.file_sets@ == b.file_sets@,
        a.dependencies@ == b.dependencies@,
        ao.len <= bo.len + 2,
        a.nodes@[i as int].last_cmd == b.nodes@[i as int].last_cmd,
        retirement::selected(fact) ==> a.next_question == b.next_question,
        retirement::selected(fact) ==> a.dependencies@ == b.dependencies@,
        retirement::selected(fact) ==> a.nodes@[i as int]
            == retirement::fact_node(b.nodes@[i as int], fact),
        retirement::selected(fact) && retirement::notices_only(bo)
            ==> retirement::notices_only(ao),
        fact is Priority ==> ao == bo,
        match fact {
            Fact::PrMerged { head, checks_passed } => b.nodes@[i as int].had_pr
                && b.nodes@[i as int].merged.is_none()
                && !(b.nodes@[i as int].phase is Closed)
                && !(b.nodes@[i as int].approved == Some(head) && checks_passed)
                ==> retirement::unapproved_note(ao, i),
            _ => true,
        },
    ensures post(b, a, i, fact, bo, ao),
{ reveal(post); }

pub(super) proof fn unpack(b: World, a: World, i: usize, fact: Fact, bo: Outbox, ao: Outbox)
    requires post(b, a, i, fact, bo, ao),
    ensures (fact == Fact::IncidentCleared { by_rob: true }
            && b.nodes@[i as int].waiting_incident && b.nodes@[i as int].effect.is_none()
            && (!b.nodes@[i as int].paused || b.nodes@[i as int].phase is Stopping)
            && !(b.nodes@[i as int].phase is Closed)
            ==> distance(a.nodes@[i as int]) < distance(b.nodes@[i as int])),
        (b.nodes@[i as int].phase is Closed ==> a.nodes@[i as int].phase is Closed),
        a.nodes@[i as int].repo == b.nodes@[i as int].repo,
        a.nodes@[i as int].footprint == b.nodes@[i as int].footprint,
        (settle_shape(b.nodes@[i as int]) ==> settle_shape(a.nodes@[i as int])),
        a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i
            ==> a.nodes@[j] == b.nodes@[j],
        a.nodes@[i as int].generation as int <= b.nodes@[i as int].generation as int + 1,
        a.nodes@[i as int].next_seq == b.nodes@[i as int].next_seq,
        a.nodes@[i as int].credit.start == b.nodes@[i as int].credit.start,
        a.nodes@[i as int].credit.earned == b.nodes@[i as int].credit.earned,
        a.nodes@[i as int].credit.added == b.nodes@[i as int].credit.added,
        a.nodes@[i as int].credit.spent as int <= b.nodes@[i as int].credit.spent as int + 1,
        a.next_ticket == b.next_ticket,
        a.next_question as int <= b.next_question as int + 1,
        a.gates@ == b.gates@,
        a.pool_capacity == b.pool_capacity,
        a.fail_fast == b.fail_fast,
        a.file_sets@ == b.file_sets@,
        a.dependencies@ == b.dependencies@,
        ao.len <= bo.len + 2,
        a.nodes@[i as int].last_cmd == b.nodes@[i as int].last_cmd,
        retirement::selected(fact) ==> a.next_question == b.next_question,
        retirement::selected(fact) ==> a.dependencies@ == b.dependencies@,
        retirement::selected(fact) ==> a.nodes@[i as int]
            == retirement::fact_node(b.nodes@[i as int], fact),
        retirement::selected(fact) && retirement::notices_only(bo)
            ==> retirement::notices_only(ao),
        fact is Priority ==> ao == bo,
        match fact {
            Fact::PrMerged { head, checks_passed } => b.nodes@[i as int].had_pr
                && b.nodes@[i as int].merged.is_none()
                && !(b.nodes@[i as int].phase is Closed)
                && !(b.nodes@[i as int].approved == Some(head) && checks_passed)
                ==> retirement::unapproved_note(ao, i),
            _ => true,
        },
{ reveal(post); }

pub(super) proof fn inline_fields(n: Node, fact: Fact)
    requires matches!(fact, Fact::Priority(_) | Fact::Conflict | Fact::PrClosedByPerson),
    ensures {
        let expected = match fact {
            Fact::Priority(priority) => if n.phase is Closed { n } else { Node { priority, ..n } },
            Fact::Conflict => if matches!(n.phase, Phase::Publishing | Phase::Queued) {
                retirement::phased(n, Phase::Rebasing)
            } else { n },
            Fact::PrClosedByPerson => if !n.pr_open { n } else {
                let closed_pr = Node { pr_open: false, ..n };
                if n.phase is Stopping || n.phase is Closed { closed_pr }
                else { retirement::stopped(closed_pr, Outcome::Stopped) }
            },
            _ => n,
        };
        retirement::fact_node(n, fact).phase == expected.phase
            && retirement::fact_node(n, fact).pos == expected.pos
            && retirement::fact_node(n, fact).repo == expected.repo
            && retirement::fact_node(n, fact).footprint == expected.footprint
            && retirement::fact_node(n, fact).generation == expected.generation
            && retirement::fact_node(n, fact).next_seq == expected.next_seq
            && retirement::fact_node(n, fact).effect == expected.effect
            && retirement::fact_node(n, fact).run == expected.run
            && retirement::fact_node(n, fact).job == expected.job
            && retirement::fact_node(n, fact).pool == expected.pool
            && retirement::fact_node(n, fact).ticket == expected.ticket
            && retirement::fact_node(n, fact).holds == expected.holds
            && retirement::fact_node(n, fact).paused == expected.paused
            && retirement::fact_node(n, fact).last_cmd == expected.last_cmd
            && retirement::fact_node(n, fact).question == expected.question
            && retirement::fact_node(n, fact).attempt == expected.attempt
            && retirement::fact_node(n, fact).round == expected.round
            && retirement::fact_node(n, fact).replanned == expected.replanned
            && retirement::fact_node(n, fact).plan_retries == expected.plan_retries
            && retirement::fact_node(n, fact).audit_rounds == expected.audit_rounds
            && retirement::fact_node(n, fact).early_end == expected.early_end
            && retirement::fact_node(n, fact).run_failures == expected.run_failures
            && retirement::fact_node(n, fact).run_incidents == expected.run_incidents
            && retirement::fact_node(n, fact).fx_failures == expected.fx_failures
            && retirement::fact_node(n, fact).fx_incidents == expected.fx_incidents
            && retirement::fact_node(n, fact).waiting_incident == expected.waiting_incident
            && retirement::fact_node(n, fact).incident_needs_rob == expected.incident_needs_rob
            && retirement::fact_node(n, fact).proposed == expected.proposed
            && retirement::fact_node(n, fact).adopted == expected.adopted
            && retirement::fact_node(n, fact).head == expected.head
            && retirement::fact_node(n, fact).checked == expected.checked
            && retirement::fact_node(n, fact).approved == expected.approved
            && retirement::fact_node(n, fact).merged == expected.merged
            && retirement::fact_node(n, fact).pr_open == expected.pr_open
            && retirement::fact_node(n, fact).had_pr == expected.had_pr
            && retirement::fact_node(n, fact).worktree == expected.worktree
            && retirement::fact_node(n, fact).rechecks == expected.rechecks
            && retirement::fact_node(n, fact).review_restarted == expected.review_restarted
            && retirement::fact_node(n, fact).credit == expected.credit
            && retirement::fact_node(n, fact).priority == expected.priority
            && retirement::fact_node(n, fact).review_stop == expected.review_stop
            && retirement::fact_node(n, fact).planning_started == expected.planning_started
            && retirement::fact_node(n, fact).waiting_prerequisite == expected.waiting_prerequisite
    },
{ reveal(retirement::fact_node); }

pub(super) proof fn retry_consequences(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(),
        !(before.nodes@[i as int].phase is Closed),
        retry_bounds(before, after, i as int, out_before.len, out_after.len),
    ensures
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        (settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1,
        after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq,
        after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start,
        after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned,
        after.nodes@[i as int].credit.added == before.nodes@[i as int].credit.added,
        after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1,
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        out_after.len <= out_before.len + 2,
        after.nodes@[i as int].last_cmd == before.nodes@[i as int].last_cmd,
{
    facts_ci_failed_lemmas::retry_frame(before, after, i, out_before, out_after);
    frame_consequences(before, after, i, out_before, out_after);
}

}
