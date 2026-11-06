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

/// #806 fact rows: the exact unchanged fact_transition conclusions as one opaque bundle.
/// Each dispatch arm proves the bundle in its own scope; the join only needs the bundle.
#[verifier::opaque]
pub(super) open spec fn post(b: World, a: World, i: usize, fact: Fact, bo: Outbox, ao: Outbox) -> bool {
    ((fact == Fact::IncidentCleared { by_rob: true } && b.nodes@[i as int].waiting_incident && b.nodes@[i as int].effect.is_none() && (!b.nodes@[i as int].paused || b.nodes@[i as int].phase is Stopping) && !(b.nodes@[i as int].phase is Closed) ==> distance(a.nodes@[i as int]) < distance(b.nodes@[i as int])))
        && ((b.nodes@[i as int].phase is Closed ==> a.nodes@[i as int].phase is Closed))
        && (a.nodes@[i as int].repo == b.nodes@[i as int].repo)
        && (a.nodes@[i as int].footprint == b.nodes@[i as int].footprint)
        && ((settle_shape(b.nodes@[i as int]) ==> settle_shape(a.nodes@[i as int])))
        && (a.nodes@.len() == b.nodes@.len())
        && (forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j])
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
        && (retirement::selected(fact) ==> a.nodes@[i as int] == retirement::fact_node(b.nodes@[i as int], fact))
        && (retirement::selected(fact) && retirement::notices_only(bo) ==> retirement::notices_only(ao))
        && (fact is Priority ==> ao == bo)
        && (match fact { Fact::PrMerged { head, checks_passed } => b.nodes@[i as int].had_pr && b.nodes@[i as int].merged.is_none() && !(b.nodes@[i as int].phase is Closed) && !(b.nodes@[i as int].approved == Some(head) && checks_passed) ==> retirement::unapproved_note(ao, i), _ => true, })
        && (retirement_retry::selected(fact) ==> a.nodes@[i as int] == retirement_retry::fact_result(b.nodes@[i as int], fact, b.fail_fast, b.next_question).0)
        && (retirement_retry::selected(fact) ==> a.next_question as int == b.next_question as int + if retirement_retry::fact_result(b.nodes@[i as int], fact, b.fail_fast, b.next_question).1 { 1int } else { 0int })
        && (retirement_retry::selected(fact) && retirement::notices_only(bo) ==> retirement::notices_only(ao))
        && (retirement_retry::selected(fact) ==> a.dependencies@ == b.dependencies@)
        && (match fact { Fact::CiFailed { infra } => retirement_runtime::ci_post(b.nodes@[i as int], a.nodes@[i as int], infra, b.fail_fast, b.next_question, a.next_question) && retirement_runtime::ci_output(b.nodes@[i as int], infra, b.fail_fast, bo, ao), _ => true, })
}

/// #806 fact rows: restate the bundle as the original postconditions, verbatim.
pub(super) proof fn unfold(b: World, a: World, i: usize, fact: Fact, bo: Outbox, ao: Outbox)
    requires post(b, a, i, fact, bo, ao),
    ensures (fact == Fact::IncidentCleared { by_rob: true } && b.nodes@[i as int].waiting_incident && b.nodes@[i as int].effect.is_none() && (!b.nodes@[i as int].paused || b.nodes@[i as int].phase is Stopping) && !(b.nodes@[i as int].phase is Closed) ==> distance(a.nodes@[i as int]) < distance(b.nodes@[i as int])),
        (b.nodes@[i as int].phase is Closed ==> a.nodes@[i as int].phase is Closed),
        a.nodes@[i as int].repo == b.nodes@[i as int].repo,
        a.nodes@[i as int].footprint == b.nodes@[i as int].footprint,
        (settle_shape(b.nodes@[i as int]) ==> settle_shape(a.nodes@[i as int])),
        a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j],
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
        retirement::selected(fact) ==> a.nodes@[i as int] == retirement::fact_node(b.nodes@[i as int], fact),
        retirement::selected(fact) && retirement::notices_only(bo) ==> retirement::notices_only(ao),
        fact is Priority ==> ao == bo,
        match fact { Fact::PrMerged { head, checks_passed } => b.nodes@[i as int].had_pr && b.nodes@[i as int].merged.is_none() && !(b.nodes@[i as int].phase is Closed) && !(b.nodes@[i as int].approved == Some(head) && checks_passed) ==> retirement::unapproved_note(ao, i), _ => true, },
        retirement_retry::selected(fact) ==> a.nodes@[i as int] == retirement_retry::fact_result(b.nodes@[i as int], fact, b.fail_fast, b.next_question).0,
        retirement_retry::selected(fact) ==> a.next_question as int == b.next_question as int + if retirement_retry::fact_result(b.nodes@[i as int], fact, b.fail_fast, b.next_question).1 { 1int } else { 0int },
        retirement_retry::selected(fact) && retirement::notices_only(bo) ==> retirement::notices_only(ao),
        retirement_retry::selected(fact) ==> a.dependencies@ == b.dependencies@,
        match fact { Fact::CiFailed { infra } => retirement_runtime::ci_post(b.nodes@[i as int], a.nodes@[i as int], infra, b.fail_fast, b.next_question, a.next_question) && retirement_runtime::ci_output(b.nodes@[i as int], infra, b.fail_fast, bo, ao), _ => true, },
{
    reveal(post);
}
}
