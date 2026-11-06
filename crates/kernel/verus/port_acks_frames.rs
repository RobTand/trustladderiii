//! #806: per-ack write sets, proved separately from checker expansion.
use super::port_acks_specs::*;
use super::*;

verus! {
/// State-table Confirmed frame: only delivery/cleanup fields and adoption agenda state change.
pub(super) proof fn confirmed_frame(n: Node, q: u64, seq: u64)
    ensures {
        let a = ack_node(n, q, seq, Ack::Confirmed);
        a == Node { effect: a.effect, fx_failures: a.fx_failures, fx_incidents: a.fx_incidents,
            run: a.run, pool: a.pool, job: a.job, pr_open: a.pr_open, worktree: a.worktree,
            adopted: a.adopted, head: a.head, checked: a.checked, generation: a.generation,
            phase: a.phase, pos: a.pos, ticket: a.ticket, ..n }
    },
{
    reveal(ack_node);
    if let Some(e) = n.effect {
        if e.seq == seq && !e.cancelling {
            confirmed_inner_frame(Node { effect: None, ..n }, e.kind);
        }
    }
}

/// State-table Retryable frame: retry delivery or release the incident's existing resources.
pub(super) proof fn retryable_frame(n: Node, q: u64, seq: u64)
    ensures {
        let a = ack_node(n, q, seq, Ack::Retryable);
        a == Node { effect: a.effect, next_seq: a.next_seq,
            fx_failures: a.fx_failures, fx_incidents: a.fx_incidents,
            run: a.run, job: a.job, pool: a.pool, pos: a.pos, generation: a.generation,
            waiting_incident: a.waiting_incident, incident_needs_rob: a.incident_needs_rob,
            ticket: a.ticket, holds: a.holds, phase: a.phase,
            worktree: a.worktree, pr_open: a.pr_open, had_pr: a.had_pr, ..n }
    },
{
    reveal(ack_node);
    if let Some(e) = n.effect {
        if e.seq == seq && !e.cancelling {
            retry_inner_frame(Node { effect: None, ..n }, e.kind);
        }
    }
}

/// State-table Permanent frame: release only the failed operation and install its owner question.
pub(super) proof fn permanent_frame(n: Node, q: u64, seq: u64)
    ensures {
        let a = ack_node(n, q, seq, Ack::Permanent);
        a == Node { effect: a.effect, question: a.question, run: a.run, job: a.job,
            pool: a.pool, holds: a.holds, phase: a.phase, generation: a.generation,
            pos: a.pos, ticket: a.ticket, ..n }
    },
{
    reveal(ack_node);
    if let Some(e) = n.effect {
        if e.seq == seq && !e.cancelling {
            permanent_inner_frame(Node { effect: None, ..n }, q, e.kind);
        }
    }
}

/// #629 body refusal frame: the permanent frame plus the existing review-stop state.
pub(super) proof fn body_refused_frame(n: Node, q: u64, seq: u64)
    ensures {
        let a = ack_node(n, q, seq, Ack::BodyPolicyRefused);
        a == Node { effect: a.effect, question: a.question, run: a.run, job: a.job,
            pool: a.pool, holds: a.holds, phase: a.phase, generation: a.generation,
            pos: a.pos, ticket: a.ticket, review_stop: a.review_stop,
            waiting_incident: a.waiting_incident, incident_needs_rob: a.incident_needs_rob, ..n }
    },
{
    reveal(ack_node);
    if let Some(e) = n.effect {
        if e.seq == seq && !e.cancelling {
            let cleared = Node { effect: None, ..n };
            if e.kind == EffectKind::OpenPr {
                stopped_inner_frame(Node { review_stop: Some(ReviewStop::PublicationBody), ..cleared });
            } else { permanent_inner_frame(cleared, q, e.kind); }
        }
    }
}
}

verus! {


/// State-table Confirmed: compose the real cleanup and agenda helper frames.
pub(super) proof fn confirmed_result(b: Node, mid: Node, a: Node, k: EffectKind)
    requires cleanup_post(b, mid, k), agenda_post(mid, a, k),
    ensures confirmed_post(b, a, k),
{ reveal(cleanup_post); reveal(agenda_post); reveal(confirmed_post); reveal(confirmed_node); }








}

verus! {
/// #806: isolate the cleanup write set before composing the acknowledgement frame.
pub(super) proof fn cleanup_inner_frame(n: Node, k: EffectKind)
    ensures {
        let a = confirmed_cleanup(n, k);
        a == Node { fx_failures: a.fx_failures, fx_incidents: a.fx_incidents, run: a.run, pool: a.pool, job: a.job, pr_open: a.pr_open, worktree: a.worktree, ..n }
    },
{ reveal(confirmed_cleanup); }

/// #806: isolate the agenda write set before composing the acknowledgement frame.
pub(super) proof fn agenda_inner_frame(n: Node, k: EffectKind)
    ensures {
        let a = confirmed_agenda_node(n, k);
        a == Node { adopted: a.adopted, head: a.head, checked: a.checked, generation: a.generation, phase: a.phase, pos: a.pos, ticket: a.ticket, effect: a.effect, ..n }
    },
{ reveal(confirmed_agenda_node); }

/// #806: isolate the confirmed write set before composing the acknowledgement frame.
pub(super) proof fn confirmed_inner_frame(n: Node, k: EffectKind)
    ensures {
        let a = confirmed_node(n, k);
        a == Node { effect: a.effect, fx_failures: a.fx_failures, fx_incidents: a.fx_incidents, run: a.run, pool: a.pool, job: a.job, pr_open: a.pr_open, worktree: a.worktree, adopted: a.adopted, head: a.head, checked: a.checked, generation: a.generation, phase: a.phase, pos: a.pos, ticket: a.ticket, ..n }
    },
{ reveal(confirmed_node); cleanup_inner_frame(n, k); agenda_inner_frame(confirmed_cleanup(n, k), k); }

/// #806: isolate the delivery write set before composing the acknowledgement frame.
pub(super) proof fn delivery_inner_frame(n: Node, k: EffectKind)
    ensures {
        let a = failed_delivery(n, k);
        a == Node { fx_failures: a.fx_failures, fx_incidents: a.fx_incidents, run: a.run, job: a.job, pool: a.pool, effect: a.effect, pos: a.pos, generation: a.generation, ..n }
    },
{ reveal(failed_delivery); }

/// #806: isolate the emitted write set before composing the acknowledgement frame.
pub(super) proof fn emitted_inner_frame(n: Node, k: EffectKind)
    ensures {
        let a = emitted_node(n, k);
        a == Node { next_seq: a.next_seq, effect: a.effect, worktree: a.worktree, pr_open: a.pr_open, had_pr: a.had_pr, ..n }
    },
{ reveal(emitted_node); }

/// #806: isolate the incident write set before composing the acknowledgement frame.
pub(super) proof fn incident_inner_frame(n: Node, needs: bool)
    ensures {
        let a = retirement::incident(n, needs);
        a == Node { waiting_incident: a.waiting_incident, incident_needs_rob: a.incident_needs_rob, ticket: a.ticket, holds: a.holds, phase: a.phase, generation: a.generation, pos: a.pos, effect: a.effect, ..n }
    },
{ reveal(retirement::incident); }

/// #806: isolate the retry write set before composing the acknowledgement frame.
pub(super) proof fn retry_inner_frame(n: Node, k: EffectKind)
    ensures {
        let a = retry_node(n, k);
        a == Node { effect: a.effect, next_seq: a.next_seq, fx_failures: a.fx_failures, fx_incidents: a.fx_incidents, run: a.run, job: a.job, pool: a.pool, pos: a.pos, generation: a.generation, waiting_incident: a.waiting_incident, incident_needs_rob: a.incident_needs_rob, ticket: a.ticket, holds: a.holds, phase: a.phase, worktree: a.worktree, pr_open: a.pr_open, had_pr: a.had_pr, ..n }
    },
{ reveal(retry_node); delivery_inner_frame(n, k); if n.fx_failures == 0 { emitted_inner_frame(failed_delivery(n, k), k); } else { incident_inner_frame(failed_delivery(n, k), failed_delivery(n, k).fx_incidents >= 2); } }

/// #806: isolate the permanent write set before composing the acknowledgement frame.
pub(super) proof fn permanent_inner_frame(n: Node, q: u64, k: EffectKind)
    ensures {
        let a = permanent_node(n, q, k);
        a == Node { effect: a.effect, question: a.question, run: a.run, job: a.job, pool: a.pool, holds: a.holds, phase: a.phase, generation: a.generation, pos: a.pos, ticket: a.ticket, ..n }
    },
{ reveal(permanent_node); }

/// #806: isolate the stopped write set before composing the acknowledgement frame.
pub(super) proof fn stopped_inner_frame(n: Node)
    ensures {
        let a = retirement::stopped(n, Outcome::Stopped);
        a == Node { question: a.question, waiting_incident: a.waiting_incident, incident_needs_rob: a.incident_needs_rob, phase: a.phase, generation: a.generation, pos: a.pos, ticket: a.ticket, effect: a.effect, ..n }
    },
{ reveal(retirement::stopped); }

}

verus! {






/// State-table Permanent outputs: compose the two actual fixed-buffer updates, including unused slots.
pub(super) proof fn two_notices_result(ob: Outbox, mid: Outbox, oa: Outbox, i: usize, q: u64)
    requires ob.len as int + 2 <= OUTBOX,
        mid.len as int == ob.len as int + 1,
        mid.items@ == ob.items@.update(ob.len as int,
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: true } })),
        oa.len as int == mid.len as int + 1,
        oa.items@ == mid.items@.update(mid.len as int,
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: q } })),
    ensures appended(ob, oa, permanent_emissions(q, i)),
{
    reveal(appended);
    assert forall|j: int| 0 <= j < OUTBOX implies #[trigger] oa.items@[j] ==
        if ob.len <= j < ob.len + 2 { Some(permanent_emissions(q, i)[j - ob.len]) }
        else { ob.items@[j] } by {}
}
}

verus! {}

verus! {
/// State-table Ack stale/cancelled rows: derive the exact local stutter/clear frame.
pub(super) proof fn inactive_ack(b: Node, a: Node, q: u64, i: usize, seq: u64,
    ack: Ack, ob: Outbox, oa: Outbox)
    requires oa == ob,
        match b.effect {
            Some(e) => if e.seq != seq { a == b } else { e.cancelling && a == (Node { effect: None, ..b }) },
            None => a == b,
        },
    ensures ack_local(b, a, q, q, i, seq, ack, ob, oa),
{
    reveal(ack_local); reveal(ack_node); reveal(ack_output); reveal(ack_emissions); reveal(appended);
}

/// State-table live Ack rows: compose only opaque callee results, outside the progress proof.
pub(super) proof fn live_ack(b: Node, cleared: Node, a: Node, qb: u64, qa: u64,
    i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, !e.cancelling,
        cleared_post(b, cleared),
        match ack {
            Ack::Confirmed => confirmed_post(cleared, a, e.kind) && qa == qb && oa == ob,
            Ack::Retryable => retry_post(cleared, a, e.kind)
                && retry_output(cleared, i, e.kind, ob, oa) && qa == qb,
            Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr =>
                a == retirement::stopped(Node { review_stop: Some(ReviewStop::PublicationBody), ..cleared }, Outcome::Stopped)
                && qa == qb && oa == ob,
            _ => permanent_post(cleared, a, qb, e.kind)
                && appended(ob, oa, permanent_emissions(qb, i)) && qa as int == qb as int + 1,
        },
    ensures ack_local(b, a, qb, qa, i, seq, ack, ob, oa),
{
    reveal(cleared_post); reveal(ack_local); reveal(ack_node); reveal(ack_output); reveal(ack_emissions);
    reveal(confirmed_post); reveal(permanent_post); reveal(retry_post);
    if ack == Ack::Retryable { reveal(retry_output); }
    if oa == ob { reveal(appended); }
}

/// #806: compose the real Ack local result with unchanged world storage.
pub(super) proof fn finish_ack(b: World, a: World, i: usize, seq: u64, ack: Ack,
    ob: Outbox, oa: Outbox)
    requires i < b.nodes@.len(), a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j],
        a.next_ticket == b.next_ticket, a.gates@ == b.gates@,
        a.file_sets@ == b.file_sets@, a.dependencies@ == b.dependencies@,
        a.pool_capacity == b.pool_capacity, a.fail_fast == b.fail_fast,
        ack_local(b.nodes@[i as int], a.nodes@[i as int], b.next_question, a.next_question, i, seq, ack, ob, oa),
    ensures ack_post(b, a, i, seq, ack, ob, oa),
{ reveal(ack_local); reveal(ack_post); reveal(world_frame); }
}

verus! {
/// State-table Confirmed: agenda shape bounds used before the real position increment.
pub(super) proof fn agenda_input(n: Node)
    ensures current_item(n).is_some() ==> n.pos < 5,
{}

/// #805 / #806: derive the original Confirmed-agenda contract from its exact local frame.
pub(super) proof fn agenda_contract(b: Node, a: Node, kind: EffectKind)
    requires b.generation < u64::MAX, kind == EffectKind::StopRun ==> !b.pool,
        agenda_post(b, a, kind),
    ensures (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(a) == distance(b)),
        (b.effect.is_none() && b.question.is_none() && !b.waiting_incident
            && (!b.paused || b.phase is Stopping)
            && (current_item(b) == Some(Item::Do(kind))
                || current_item(b) == Some(Item::Teardown(kind)))
            ==> distance(a) + 2 <= distance(b)),
        a.repo == b.repo, a.footprint == b.footprint,
        (settle_shape(b)
            && (b.phase is Stopping ==> current_item(b) == Some(Item::Teardown(kind)))
            ==> settle_shape(a)),
        a.generation as int <= b.generation as int + 1,
        a.generation >= b.generation,
        a.next_seq == b.next_seq, a.credit == b.credit,
        a.question == b.question,
        a.fx_failures == b.fx_failures,
        a.fx_incidents == b.fx_incidents,
        a.last_cmd == b.last_cmd,
{
    reveal(agenda_post); reveal(confirmed_agenda_node); reveal(distance);
}

/// #805 / #806: preserve the original Confirmed progress and frame contracts by composition.
pub(super) proof fn confirmed_contract(b: Node, mid: Node, a: Node, kind: EffectKind)
    requires b.generation < u64::MAX, cleanup_post(b, mid, kind),
        (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(a) == distance(mid)),
        (mid.effect.is_none() && mid.question.is_none() && !mid.waiting_incident
            && (!mid.paused || mid.phase is Stopping)
            && (current_item(mid) == Some(Item::Do(kind))
                || current_item(mid) == Some(Item::Teardown(kind)))
            ==> distance(a) + 2 <= distance(mid)),
        a.repo == mid.repo, a.footprint == mid.footprint,
        (settle_shape(mid)
            && (mid.phase is Stopping ==> current_item(mid) == Some(Item::Teardown(kind)))
            ==> settle_shape(a)),
        a.generation as int <= mid.generation as int + 1,
        a.generation >= mid.generation,
        a.next_seq == mid.next_seq, a.credit == mid.credit,
        a.question == mid.question,
        a.fx_failures == mid.fx_failures,
        a.fx_incidents == mid.fx_incidents,
        a.last_cmd == mid.last_cmd,
    ensures (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(a) == distance(b)),
        (b.effect.is_none() && b.question.is_none() && !b.waiting_incident
            && (!b.paused || b.phase is Stopping)
            && (current_item(b) == Some(Item::Do(kind))
                || current_item(b) == Some(Item::Teardown(kind)))
            ==> distance(a) + 2 <= distance(b)),
        a.repo == b.repo, a.footprint == b.footprint,
        (settle_shape(b)
            && (b.phase is Stopping ==> current_item(b) == Some(Item::Teardown(kind)))
            ==> settle_shape(a)),
        a.generation as int <= b.generation as int + 1,
        a.generation >= b.generation,
        a.next_seq == b.next_seq, a.credit == b.credit,
        a.question == b.question,
        a.fx_failures == 0, a.fx_incidents == 0,
        a.last_cmd == b.last_cmd,
{
    reveal(cleanup_post); reveal(confirmed_cleanup);
    cleanup_lemmas::resource_distance(b, mid);
}
}

verus! {
/// State-table first Retryable: compose the real counter frame and exact emitter payload.
pub(super) proof fn retry_emitted_result(b: Node, mid: Node, a: Node, k: EffectKind,
    ob: Outbox, oa: Outbox, i: usize)
    requires b.fx_failures == 0, delivery_post(b, mid, k),
        a == (Node { next_seq: (mid.next_seq + 1) as u64,
            effect: Some(InFlight { seq: mid.next_seq, kind: k, cancelling: false }),
            worktree: mid.worktree || k == EffectKind::PrepareWorktree,
            pr_open: mid.pr_open || k == EffectKind::OpenPr,
            had_pr: mid.had_pr || k == EffectKind::OpenPr, ..mid }),
        port_review_output::effect_post(ob.items@, ob.len, oa.items@, mid, i, k),
        oa.len as int == ob.len as int + 1, ob.len < OUTBOX,
    ensures retry_post(b, a, k), retry_output(b, i, k, ob, oa),
{
    reveal(delivery_post); reveal(failed_delivery); reveal(retry_post); reveal(retry_node); reveal(emitted_node);
    reveal(retry_output); reveal(appended);
    reveal(port_review_output::effect_post); reveal(port_review_output::selected_head);
}

/// State-table second Retryable: compose the real incident frame and notice-only output guarantee.
pub(super) proof fn retry_incident_result(b: Node, mid: Node, a: Node, k: EffectKind,
    ob: Outbox, oa: Outbox, i: usize, needs: bool)
    requires b.fx_failures == 1, delivery_post(b, mid, k), needs == (mid.fx_incidents >= 2),
        a == retirement::incident(mid, needs), oa.len as int == ob.len as int + 1,
        retirement::notices_only(ob) ==> retirement::notices_only(oa),
    ensures retry_post(b, a, k), retry_output(b, i, k, ob, oa),
{
    reveal(delivery_post); reveal(retry_post); reveal(retry_node); reveal(retry_output);
}
}

verus! {
/// #805 / #806: a cancelled Confirmed acknowledgement consumes its sole delivery rank.
pub(super) proof fn cancelled_ack_progress(w: World, b: Node, a: Node, e: InFlight)
    requires b.effect == Some(e), e.cancelling, cleared_post(b, a),
    ensures node_checked(w, b) && (!b.paused || b.phase is Stopping)
        ==> distance(a) < distance(b),
{ reveal(cleared_post); reveal(distance); }

/// #805 / #806: forward the original Confirmed callee rank bounds after clearing the real effect.
pub(super) proof fn confirmed_ack_progress(w: World, b: Node, c: Node, a: Node, e: InFlight)
    requires b.effect == Some(e), !e.cancelling, cleared_post(b, c),
        matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(a) == distance(c),
        c.effect.is_none() && c.question.is_none() && !c.waiting_incident
            && (!c.paused || c.phase is Stopping)
            && (current_item(c) == Some(Item::Do(e.kind)) || current_item(c) == Some(Item::Teardown(e.kind)))
            ==> distance(a) + 2 <= distance(c),
    ensures node_checked(w, b) && (!b.paused || b.phase is Stopping)
        ==> distance(a) < distance(b),
{ reveal(cleared_post); reveal(distance); }
}

verus! {
/// #806: clearing delivery bookkeeping preserves the real current agenda item.
pub(super) proof fn ack_agenda_frame(b: Node, a: Node)
    requires a.phase == b.phase, a.pos == b.pos,
    ensures current_item(a) == current_item(b),
{}
}

verus! {
/// State-table Permanent / #806: compose two opaque real call frames after the continuation match.
pub(super) proof fn permanent_composed(b: Node, mid: Node, a: Node, q: u64, k: EffectKind, cont: Cont)
    requires !(b.phase is Stopping), cont == failed_cont(b),
        permanent_abandon_post(b, mid, k), asked_post(mid, a, q, cont),
    ensures permanent_post(b, a, q, k),
{ reveal(permanent_abandon_post); reveal(asked_post); reveal(permanent_post); reveal(permanent_node); }
}

verus! {
/// State-table confirmed adoption / #806: compose local adoption with the real phase setter.
pub(super) proof fn agenda_adoption_composed(b: Node, mid: Node, a: Node, k: EffectKind)
    requires k == EffectKind::PrepareWorktree, !b.adopted, b.proposed.is_some(),
        current_item(b) == Some(Item::Do(k)) || current_item(b) == Some(Item::Teardown(k)),
        adoption_post(b, mid), a == (Node { generation: (mid.generation + 1) as u64,
            phase: Phase::Checking, pos: 0, ticket: None,
            effect: match mid.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }, ..mid }),
    ensures agenda_post(b, a, k),
{ reveal(adoption_post); reveal(agenda_post); reveal(confirmed_agenda_node); reveal(retirement::phased); }

/// State-table confirmed ordinary agenda / #806: consume exactly one matching non-start item.
pub(super) proof fn agenda_incremented(b: Node, a: Node, k: EffectKind)
    requires !matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase),
        current_item(b) == Some(Item::Do(k)) || current_item(b) == Some(Item::Teardown(k)),
        !(k == EffectKind::PrepareWorktree && !b.adopted && b.proposed.is_some()),
        handled_increment(b, a),
    ensures agenda_post(b, a, k),
{ reveal(handled_increment); reveal(agenda_post); reveal(confirmed_agenda_node); }

/// State-table confirmed unmatched agenda / #806: no item is consumed.
pub(super) proof fn agenda_stutter(b: Node, k: EffectKind)
    requires matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
        || !(current_item(b) == Some(Item::Do(k)) || current_item(b) == Some(Item::Teardown(k))),
    ensures agenda_post(b, b, k),
{ reveal(agenda_post); reveal(confirmed_agenda_node); }
}

verus! {
/// State-table cancelled Ack / #806: compose the clearing stage without re-expanding its assignment.
pub(super) proof fn cancelled_ack(b: Node, a: Node, q: u64, i: usize, seq: u64,
    ack: Ack, ob: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, e.cancelling, cleared_post(b, a),
    ensures ack_local(b, a, q, q, i, seq, ack, ob, ob),
{
    reveal(cleared_post);
    inactive_ack(b, a, q, i, seq, ack, ob, ob);
}
}

verus! {
/// #771 / #806: the ordinary agenda only changes pos, so resource shape follows its real item.
pub(super) proof fn agenda_ordinary_shape(b: Node, a: Node, k: EffectKind)
    requires a.phase == b.phase, a.pool == b.pool, a.holds == b.holds,
        k == EffectKind::StopRun ==> !b.pool,
        a.pos == if !matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            && (current_item(b) == Some(Item::Do(k)) || current_item(b) == Some(Item::Teardown(k))) {
                (b.pos + 1) as u8
            } else { b.pos },
    ensures settle_shape(b) && (b.phase is Stopping ==> current_item(b) == Some(Item::Teardown(k)))
        ==> settle_shape(a),
{}
}

verus! {
/// #806 Confirmed row: specialize acknowledgement composition before entering the dispatcher.
pub(super) proof fn confirmed_live(b: Node, c: Node, a: Node, q: u64,
    i: usize, seq: u64, ob: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, !e.cancelling,
        cleared_post(b, c), confirmed_post(c, a, e.kind),
    ensures ack_local(b, a, q, q, i, seq, Ack::Confirmed, ob, ob),
{ live_ack(b, c, a, q, q, i, seq, Ack::Confirmed, ob, ob, e); }

/// #806 Retryable row: specialize acknowledgement composition and its output equation.
pub(super) proof fn retry_live(b: Node, c: Node, a: Node, q: u64,
    i: usize, seq: u64, ob: Outbox, oa: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, !e.cancelling,
        cleared_post(b, c), retry_post(c, a, e.kind), retry_output(c, i, e.kind, ob, oa),
    ensures ack_local(b, a, q, q, i, seq, Ack::Retryable, ob, oa),
{ live_ack(b, c, a, q, q, i, seq, Ack::Retryable, ob, oa, e); }

/// #806 Permanent/nonpublication-refusal rows: specialize the exact ask and output composition.
pub(super) proof fn permanent_live(b: Node, c: Node, a: Node, qb: u64, qa: u64,
    i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, !e.cancelling,
        ack == Ack::Permanent || (ack == Ack::BodyPolicyRefused && e.kind != EffectKind::OpenPr),
        cleared_post(b, c), permanent_post(c, a, qb, e.kind),
        appended(ob, oa, permanent_emissions(qb, i)), qa as int == qb as int + 1,
    ensures ack_local(b, a, qb, qa, i, seq, ack, ob, oa),
{ live_ack(b, c, a, qb, qa, i, seq, ack, ob, oa, e); }

/// #629 publication-refusal row: specialize the review stop with its unchanged output.
pub(super) proof fn refused_live(b: Node, c: Node, a: Node, q: u64,
    i: usize, seq: u64, ob: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, !e.cancelling, e.kind == EffectKind::OpenPr,
        cleared_post(b, c), a == retirement::stopped(Node { review_stop: Some(ReviewStop::PublicationBody), ..c }, Outcome::Stopped),
    ensures ack_local(b, a, q, q, i, seq, Ack::BodyPolicyRefused, ob, ob),
{ live_ack(b, c, a, q, q, i, seq, Ack::BodyPolicyRefused, ob, ob, e); }
}

verus! {
/// #806 adoption row: derive the requested neutral field facts from the real helper's frame.
pub(super) proof fn adoption_fields(b: Node, a: Node, k: EffectKind, reset: bool)
    requires b.generation < u64::MAX,
        if reset { confirmed_post(b, a, k) } else { agenda_post(b, a, k) },
    ensures (b.phase == Phase::Making && b.pos == 0 && b.proposed.is_some() && k == EffectKind::PrepareWorktree) ==> ((a.adopted)
            && (a.head == if b.adopted { b.head } else { b.proposed })
            && (a.phase == if b.adopted { Phase::Making } else { Phase::Checking })
            && (a.pos == if b.adopted { 1u8 } else { 0u8 })
            && (a.generation as int == b.generation as int + if b.adopted { 0int } else { 1int })
            && (a.ticket == if b.adopted { b.ticket } else { None })
            && (a.checked == if b.adopted { b.checked } else { None })
            && (a.repo == b.repo)
            && (a.footprint == b.footprint)
            && (a.next_seq == b.next_seq)
            && (a.run == b.run)
            && (a.job == b.job)
            && (a.pool == b.pool)
            && (a.holds == b.holds)
            && (a.paused == b.paused)
            && (a.last_cmd == b.last_cmd)
            && (a.question == b.question)
            && (a.attempt == b.attempt)
            && (a.round == b.round)
            && (a.replanned == b.replanned)
            && (a.plan_retries == b.plan_retries)
            && (a.audit_rounds == b.audit_rounds)
            && (a.early_end == b.early_end)
            && (a.run_failures == b.run_failures)
            && (a.run_incidents == b.run_incidents)
            && (a.waiting_incident == b.waiting_incident)
            && (a.incident_needs_rob == b.incident_needs_rob)
            && (a.proposed == b.proposed)
            && (a.approved == b.approved)
            && (a.merged == b.merged)
            && (a.pr_open == b.pr_open)
            && (a.had_pr == b.had_pr)
            && (a.worktree == b.worktree)
            && (a.rechecks == b.rechecks)
            && (a.review_restarted == b.review_restarted)
            && (a.credit == b.credit)
            && (a.priority == b.priority)
            && (a.review_stop == b.review_stop)
            && (a.planning_started == b.planning_started)
            && (a.waiting_prerequisite == b.waiting_prerequisite)
            && (a.effect == if b.adopted { b.effect } else { match b.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None } })
            && a.fx_failures == if reset { 0u8 } else { b.fx_failures }
            && a.fx_incidents == if reset { 0u8 } else { b.fx_incidents }),
{
    reveal(confirmed_post); reveal(agenda_post); reveal(confirmed_node);
    reveal(confirmed_cleanup); reveal(confirmed_agenda_node);
}

/// #806 adoption Ack row: bridge the cleared delivery to the neutral field facts.
pub(super) proof fn ack_adoption_fields(b: Node, a: Node, q: u64, i: usize, seq: u64, ob: Outbox)
    requires b.generation < u64::MAX, ack_local(b, a, q, q, i, seq, Ack::Confirmed, ob, ob),
    ensures (b.phase == Phase::Making && b.pos == 0 && b.proposed.is_some()
        && (match b.effect { Some(e) => e.kind == EffectKind::PrepareWorktree && !e.cancelling && e.seq == seq, None => false }))
        ==> ((a.adopted)
            && (a.head == if b.adopted { b.head } else { b.proposed })
            && (a.phase == if b.adopted { Phase::Making } else { Phase::Checking })
            && (a.pos == if b.adopted { 1u8 } else { 0u8 })
            && (a.generation as int == b.generation as int + if b.adopted { 0int } else { 1int })
            && (a.ticket == if b.adopted { b.ticket } else { None })
            && (a.checked == if b.adopted { b.checked } else { None })
            && (a.repo == b.repo)
            && (a.footprint == b.footprint)
            && (a.next_seq == b.next_seq)
            && (a.run == b.run)
            && (a.job == b.job)
            && (a.pool == b.pool)
            && (a.holds == b.holds)
            && (a.paused == b.paused)
            && (a.last_cmd == b.last_cmd)
            && (a.question == b.question)
            && (a.attempt == b.attempt)
            && (a.round == b.round)
            && (a.replanned == b.replanned)
            && (a.plan_retries == b.plan_retries)
            && (a.audit_rounds == b.audit_rounds)
            && (a.early_end == b.early_end)
            && (a.run_failures == b.run_failures)
            && (a.run_incidents == b.run_incidents)
            && (a.waiting_incident == b.waiting_incident)
            && (a.incident_needs_rob == b.incident_needs_rob)
            && (a.proposed == b.proposed)
            && (a.approved == b.approved)
            && (a.merged == b.merged)
            && (a.pr_open == b.pr_open)
            && (a.had_pr == b.had_pr)
            && (a.worktree == b.worktree)
            && (a.rechecks == b.rechecks)
            && (a.review_restarted == b.review_restarted)
            && (a.credit == b.credit)
            && (a.priority == b.priority)
            && (a.review_stop == b.review_stop)
            && (a.planning_started == b.planning_started)
            && (a.waiting_prerequisite == b.waiting_prerequisite)
            && (a.effect.is_none())
            && (a.fx_failures == 0)
            && (a.fx_incidents == 0)),
{
    reveal(ack_local); reveal(ack_node); reveal(confirmed_post);
    if b.phase == Phase::Making && b.pos == 0 && b.proposed.is_some() {
        if let Some(e) = b.effect {
            if e.kind == EffectKind::PrepareWorktree && !e.cancelling && e.seq == seq {
                adoption_fields(Node { effect: None, ..b }, a, e.kind, true);
            }
        }
    }
}
}

verus! {
/// #771 / #806: delivery-key clearing changes no agenda, arithmetic or settle-shape field.
pub(super) proof fn ack_clear_input(b: Node, a: Node, e: InFlight)
    requires b.effect == Some(e), cleared_post(b, a), node_headroom(b), arithmetic_consistent(b),
    ensures node_headroom(a), arithmetic_consistent(a), current_item(a) == current_item(b), settle_shape(a) == settle_shape(b),
        stopping_effect_fits(b) && !e.cancelling && a.phase is Stopping
            ==> current_item(a) == Some(Item::Teardown(e.kind)),
{ reveal(node_headroom); reveal(arithmetic_consistent); reveal(cleared_post); reveal(settle_shape); reveal(stopping_effect_fits); }
}

verus! {
/// #806 live Ack extraction: restore the original sampled effect in the helper's local frame.
pub(super) proof fn lift_live_local(b: Node, c: Node, a: Node, qb: u64, qa: u64,
    i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox, e: InFlight)
    requires b.effect == Some(e), e.seq == seq, cleared_post(b, c),
        ack_local(Node { effect: Some(e), ..c }, a, qb, qa, i, e.seq, ack, ob, oa),
    ensures ack_local(b, a, qb, qa, i, seq, ack, ob, oa),
{ reveal(cleared_post); }
}

verus! {
/// #805 / #806: derive the original Confirmed rank implication separately from dispatcher branches.
pub(super) proof fn confirmed_progress_from_local(w: World, b: Node, a: Node, q: u64,
    i: usize, seq: u64, ob: Outbox)
    requires b.generation < u64::MAX, ack_local(b, a, q, q, i, seq, Ack::Confirmed, ob, ob),
    ensures (node_checked(w, b) && (!b.paused || b.phase is Stopping)
        && (match b.effect { Some(e) => e.seq == seq, None => false })) ==> distance(a) < distance(b),
{
    reveal(ack_local); reveal(ack_node);
    if let Some(e) = b.effect {
        if e.seq == seq {
            let c = Node { effect: None, ..b };
            assert(cleared_post(b, c)) by { reveal(cleared_post); }
            if e.cancelling {
                cancelled_ack_progress(w, b, a, e);
            } else {
                reveal(confirmed_node); reveal(confirmed_cleanup);
                let mid = confirmed_cleanup(c, e.kind);
                assert(cleanup_post(c, mid, e.kind)) by { reveal(cleanup_post); }
                assert(agenda_post(mid, a, e.kind)) by { reveal(agenda_post); }
                agenda_contract(mid, a, e.kind);
                confirmed_contract(c, mid, a, e.kind);
                confirmed_ack_progress(w, b, c, a, e);
            }
        }
    }
}
}

verus! {
/// #771 / #806: establish the legacy Ack field and arithmetic bounds from the exact local result.
pub(super) proof fn ack_node_bounds(b: Node, a: Node, qb: u64, qa: u64,
    i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox)
    requires node_headroom(b), arithmetic_consistent(b),
        ack_local(b, a, qb, qa, i, seq, ack, ob, oa),
    ensures (b.phase is Closed && b.effect.is_none() ==> a.phase is Closed),
        a.repo == b.repo, a.footprint == b.footprint,
        settle_shape(b) && stopping_effect_fits(b) ==> settle_shape(a),
        a.generation as int <= b.generation as int + 2,
        a.next_seq as int <= b.next_seq as int + 1,
        a.credit == b.credit, a.last_cmd == b.last_cmd,
{
    reveal(ack_local); reveal(ack_node); reveal(confirmed_node);
    reveal(confirmed_cleanup); reveal(confirmed_agenda_node);
    reveal(retry_node); reveal(failed_delivery); reveal(emitted_node);
    reveal(permanent_node); reveal(failed_cont);
    reveal(retirement::incident); reveal(retirement::stopped);
    reveal(settle_shape); reveal(stopping_effect_fits);
}
}
