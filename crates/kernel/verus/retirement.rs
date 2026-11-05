//! #806: complete safety assertions retained when retiring bounded Kani proofs.
use super::*;

verus! {
/// #805 / #806: recording the observed merge leaves the progress metric unchanged
/// until the real handler enters Stopping. Isolate metric expansion from its full frame.
pub(super) proof fn merge_input_distance(n: Node, head: HeadId)
    ensures distance(Node { merged: Some(head), pr_open: false, ..n }) == distance(n),
{
    reveal(distance);
}

/// State-table phase changes: exact update used by the existing set_phase body.
pub(super) open spec fn phased(n: Node, phase: Phase) -> Node {
    Node { generation: (n.generation + 1) as u64, phase, pos: 0, ticket: None,
        effect: match n.effect {
            Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        }, ..n }
}

/// State-table stopping rows: changing the requested outcome preserves an
/// already-running teardown, and Closed stays final.
#[verifier::opaque]
pub(super) open spec fn stopped(n: Node, outcome: Outcome) -> Node {
    match n.phase {
        Phase::Closed(_) => n,
        Phase::Stopping(o) => Node {
            phase: Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }), ..n
        },
        _ => phased(Node { question: None, waiting_incident: false,
            incident_needs_rob: false, ..n }, Phase::Stopping(outcome)),
    }
}

/// State-table incident rows: release a holding phase's slot and retain its resume.
#[verifier::opaque]
pub(super) open spec fn incident(n: Node, needs_rob: bool) -> Node {
    let waiting = Node { waiting_incident: true, incident_needs_rob: needs_rob,
        ticket: None, ..n };
    match n.phase {
        Phase::Making => phased(Node { holds: false, ..waiting }, Phase::Ready(Resume::Making)),
        Phase::Checking => phased(Node { holds: false, ..waiting }, Phase::Ready(Resume::Checking)),
        Phase::Reviewing => phased(Node { holds: false, ..waiting }, Phase::Ready(Resume::Reviewing)),
        Phase::Publishing | Phase::Queued => phased(Node { holds: false, ..waiting }, Phase::Ready(Resume::Publishing)),
        Phase::Rebasing => phased(Node { holds: false, ..waiting }, Phase::Ready(Resume::Rebasing)),
        _ => waiting,
    }
}

/// State-table consistency rows omitted from #771's progress projection.
/// Together with node_checked this is equivalent to the REAL check_node result.
#[verifier::opaque]
pub(super) open spec fn node_rules(w: World, n: Node) -> bool {
    node_checked(w, n)
        && (n.review_stop.is_some() ==> n.phase is Stopping || n.phase is Closed)
        && (n.incident_needs_rob ==> n.waiting_incident)
        && match n.question {
            None => true,
            Some(q) => (q.cost > 0 ==> n.credit.ready)
                && (n.phase == Phase::AwaitingRob ==> {
                    (match q.cont { Cont::Ready(r) => ready_checked(n, r), _ => true })
                    && (matches!(q.cont, Cont::Auditing | Cont::Unplanned | Cont::Planning { .. }) ==> n.credit.ready)
                }),
        }
        && (n.credit.withheld ==> n.credit.ready
            && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned
                | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
            && match n.question { None => true, Some(q) => matches!(q.cont,
                Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere) })
        && (!n.credit.ready ==> n.credit.start == 0 && n.credit.earned == 0
            && n.credit.added == 0 && n.credit.spent == 0 && !n.credit.merge_credited)
        && (holding(n.phase) || n.phase is Ready || matches!(n.phase,
            Phase::Auditing | Phase::Unplanned | Phase::Planning { .. }) || n.had_pr ==> n.credit.ready)
        && (n.phase == Phase::Queued ==> n.pr_open)
        && (n.phase == Phase::Publishing && (n.pos >= 3 || (n.pos == 2
            && match n.effect { Some(e) => !e.cancelling && e.kind == EffectKind::OpenPr, None => false })) ==> n.pr_open)
        && (n.pr_open ==> n.had_pr)
        && (n.phase is Stopping ==> (n.pos > 2 ==> !n.pr_open) && (n.pos > 4 ==> !n.worktree))
        && (n.merged.is_some() ==> n.had_pr && !n.pr_open
            && matches!(n.phase, Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged)))
        && (matches!(n.phase, Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged)) ==> n.merged.is_some())
        && (n.phase == Phase::Closed(Outcome::Merged) ==> n.credit.merge_credited)
}
}

verus! {
/// #806: same two-node/two-repository construction as the retired Kani proofs.
/// No settledness, small heads, W<=3, pool<=2 or small ledger bounds are needed.
pub(super) open spec fn domain(w: World) -> bool {
    w.nodes@.len() == 2 && w.gates@.len() == 2 && w.dependencies@.len() == 0
        && !w.nodes@[0].credit.withheld && !w.nodes@[1].credit.withheld
}

/// State-table gate and pool checks, for the original two-node construction.
pub(super) open spec fn holders(w: World, repo: RepoId, end: nat) -> nat {
    (if end > 0 && w.nodes@[0].holds && w.nodes@[0].repo == repo { 1nat } else { 0nat })
        + (if end > 1 && w.nodes@[1].holds && w.nodes@[1].repo == repo { 1nat } else { 0nat })
}

/// State-table consistency: every assertion of World::check, not its #771 subset.
#[verifier::opaque]
pub(super) open spec fn accepted(w: World) -> bool {
    w.pool_capacity > 0
        && (forall|r: int| 0 <= r < 2 ==> w.gates@[r].w > 0
            && holders(w, RepoId(r as u64), 2) <= w.gates@[r].w)
        && (forall|i: int| 0 <= i < 2 ==> node_rules(w, w.nodes@[i]))
        && (w.nodes@[0].holds && w.nodes@[1].holds && w.nodes@[0].repo == w.nodes@[1].repo
            ==> footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[1].footprint))
        && (if w.nodes@[0].pool { 1int } else { 0int })
            + (if w.nodes@[1].pool { 1int } else { 0int }) <= w.pool_capacity
        && (w.nodes@[0].ticket.is_some() ==> w.nodes@[0].ticket != w.nodes@[1].ticket)
}
}

verus! {
/// State-table fact rows selected for #806 safety retirement.
pub(super) open spec fn selected(fact: Fact) -> bool {
    matches!(fact, Fact::IncidentCleared { .. } | Fact::PrDequeuedByPerson
        | Fact::CiFailed { infra: true } | Fact::PrMerged { .. }
        | Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict | Fact::UpstreamStalled)
}

/// State-table fact rows: complete node update, checked against the real handler.
#[verifier::opaque]
pub(super) open spec fn fact_node(n: Node, fact: Fact) -> Node {
    match fact {
        Fact::IncidentCleared { by_rob } => {
            if n.waiting_incident && (by_rob || !n.incident_needs_rob) {
                Node { waiting_incident: false, incident_needs_rob: false, ..n }
            } else { n }
        },
        Fact::PrDequeuedByPerson => {
            if n.phase == Phase::Queued {
                phased(Node { holds: false, paused: true, ..n }, Phase::Ready(Resume::Publishing))
            } else { n }
        },
        Fact::CiFailed { infra: true } => {
            if n.phase == Phase::Queued || (n.phase == Phase::Publishing
                && current_item(n) == Some(Item::Do(EffectKind::Enqueue))) {
                if n.run_failures == 0 {
                    phased(Node { run_failures: 1, ..n }, Phase::Publishing)
                } else {
                    let count = if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 };
                    phased(Node { run_failures: 0, run_incidents: count,
                        waiting_incident: true, incident_needs_rob: count >= 2,
                        holds: false, ..n }, Phase::Ready(Resume::Publishing))
                }
            } else { n }
        },
        Fact::Priority(priority) => {
            if n.phase is Closed { n } else { Node { priority, ..n } }
        },
        Fact::PrClosedByPerson => {
            if !n.pr_open { n } else {
                let closed_pr = Node { pr_open: false, ..n };
                if n.phase is Stopping || n.phase is Closed { closed_pr }
                else { stopped(closed_pr, Outcome::Stopped) }
            }
        },
        Fact::UpstreamStalled => {
            if n.phase != Phase::Queued || n.waiting_incident { n }
            else {
                let count = if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 };
                incident(Node { run_incidents: count, ..n }, count >= 2)
            }
        },
        Fact::Conflict => {
            if matches!(n.phase, Phase::Publishing | Phase::Queued) {
                phased(n, Phase::Rebasing)
            } else { n }
        },
        Fact::PrMerged { head, .. } => {
            if !n.had_pr || n.merged.is_some() { n }
            else { match n.phase {
                Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced) => Node {
                    merged: Some(head), phase: Phase::Closed(Outcome::Merged),
                    credit: Credit { merge_credited: true,
                        earned: if n.credit.merge_credited { n.credit.earned } else { (n.credit.earned + 1) as u64 },
                        ..n.credit }, ..n
                },
                Phase::Closed(_) => n,
                _ => stopped(Node { merged: Some(head), pr_open: false, ..n }, Outcome::Merged),
            } }
        },
        _ => n,
    }
}

/// State-table outputs: these fact handlers emit notices only.
#[verifier::opaque]
pub(super) open spec fn notices_only(out: Outbox) -> bool {
    forall|k: int| 0 <= k < OUTBOX ==> match #[trigger] out.items@[k] {
        None | Some(Out::Notice { .. }) => true, _ => false,
    }
}

/// State-table merge rows: the incident and note are observable outputs.
pub(super) open spec fn unapproved_note(out: Outbox, i: usize) -> bool {
    out.items@.contains(Some(Out::Notice { node: NodeId(i as u64),
        notice: Notice::Note(NoteKind::UnapprovedMerge) }))
}
}

verus! {
/// #806: infrastructure slice matches publishing_ci_transition's extra assumptions.
pub(super) open spec fn fact_pre(n: Node, fact: Fact) -> bool {
    match fact {
        Fact::CiFailed { infra: true } => n.phase == Phase::Publishing
            && current_item(n) == Some(Item::Do(EffectKind::Enqueue)) && !n.waiting_incident,
        _ => true,
    }
}

/// State-table consistency is insensitive to a world's Vec allocation identity.
pub(super) proof fn node_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket == after.next_ticket, before.next_question == after.next_question,
    ensures node_rules(before, n) == node_rules(after, n),
{
    reveal(node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table fact rows / #806: all per-node consistency and check_nodes rules.
/// One event, no unwind: the real handler has no loop. Counters need only one increment.
pub(super) proof fn fact_preserves_node(w: World, n: Node, fact: Fact)
    requires node_rules(w, n), selected(fact), node_headroom(n),
    ensures node_rules(w, fact_node(n, fact)),
        fact_node(n, fact).proposed == n.proposed,
        n.adopted ==> fact_node(n, fact).adopted,
        fact_node(n, fact).next_seq >= n.next_seq,
        (n.phase is Closed ==> fact_node(n, fact).phase == n.phase
            || (fact_node(n, fact).phase == Phase::Closed(Outcome::Merged)
                && matches!(n.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced)))),
        fact_node(n, fact).credit.earned as int <= n.credit.earned as int + 1,
        (fact_node(n, fact).credit.earned > n.credit.earned
            ==> !n.credit.merge_credited && fact_node(n, fact).credit.merge_credited),
        fact_node(n, fact).credit == n.credit || fact is PrMerged,
        fact_node(n, fact).review_stop == n.review_stop,
        fact_node(n, fact).credit.withheld == n.credit.withheld,
        fact_node(n, fact).attempt == n.attempt, fact_node(n, fact).replanned == n.replanned,
        (fact_node(n, fact).question.is_some() ==> fact_node(n, fact).question == n.question),
        fact_node(n, fact).phase is Planning ==> n.phase is Planning,
        !fact_node(n, fact).holds || n.holds,
        !fact_node(n, fact).pool || n.pool,
        fact_node(n, fact).ticket.is_none() || fact_node(n, fact).ticket == n.ticket,
        fact_node(n, fact).repo == n.repo, fact_node(n, fact).footprint == n.footprint,
{
    reveal(node_rules); reveal(fact_node); reveal(stopped); reveal(incident);
    match fact {
        Fact::IncidentCleared { .. } => {},
        Fact::PrDequeuedByPerson => {},
        Fact::CiFailed { infra: true } => {},
        Fact::PrMerged { .. } => {},
        Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict | Fact::UpstreamStalled => {},
        _ => {},
    }
}

/// State-table global consistency / #806: node-local facts cannot acquire
/// a slot, pool grant, conflicting footprint or duplicate ticket.
pub(super) proof fn fact_preserves_world(before: World, after: World, fact: Fact)
    requires domain(before), accepted(before), selected(fact), fact_pre(before.nodes@[0], fact),
        node_headroom(before.nodes@[0]),
        after.nodes@ == before.nodes@.update(0, fact_node(before.nodes@[0], fact)),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        after.pool_capacity == before.pool_capacity, after.dependencies@ == before.dependencies@,
    ensures domain(after), accepted(after),
{
    reveal(accepted); reveal(fact_node);
    fact_preserves_node(before, before.nodes@[0], fact);
    node_frame(before, after, after.nodes@[0]);
    node_frame(before, after, after.nodes@[1]);
    assert forall|r: int| 0 <= r < 2 implies
        holders(after, RepoId(r as u64), 2) <= after.gates@[r].w by {
        assert(holders(after, RepoId(r as u64), 2) <= holders(before, RepoId(r as u64), 2));
    }
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}

verus! {
/// #806: weaker than Kani's LIMIT/CREDIT_LIMIT; ample room to run the checker
/// after one transition. Heads, W, pool size, tickets and command IDs are unbounded.
pub(super) open spec fn bounded(w: World) -> bool {
    w.next_ticket < 1099511627776u64 && w.next_question < 1099511627776u64
        && forall|i: int| 0 <= i < w.nodes@.len() ==> {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        }
}
}

kernel_fn! {
/// State-table selected safety fact rows / #806.
/// Bound: one real apply on the original two-node construction, with no unwind.
/// Settledness is unnecessary. Full consistency, all shared Kani node assertions,
/// notice-only outputs, truthful merges and unapproved-merge notes are preserved.
pub(super) fn verify_selected_apply(mut w: World, fact: Fact) -> (result: (World, Outbox)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w),
        selected(fact), fact_pre(w.nodes@[0], fact),
    ensures domain(result.0), accepted(result.0),
        result.0.nodes@ == w.nodes@.update(0, fact_node(w.nodes@[0], fact)),
        notices_only(result.1),
        fact is Priority ==> result.1.len == 0,
        match fact {
            Fact::Conflict => {
                let n = w.nodes@[0];
                let after = result.0.nodes@[0];
                if matches!(n.phase, Phase::Publishing | Phase::Queued) {
                    after.phase == Phase::Rebasing && after.generation == n.generation + 1
                        && after.credit == n.credit && after.holds == n.holds
                } else { after == n }
            },
            Fact::PrMerged { head, checks_passed } => w.nodes@[0].had_pr
                && w.nodes@[0].merged.is_none() && !(w.nodes@[0].phase is Closed)
                ==> result.0.nodes@[0].merged == Some(head)
                    && result.0.nodes@[0].phase == Phase::Stopping(Outcome::Merged)
                    && (!(w.nodes@[0].approved == Some(head) && checks_passed)
                        ==> unapproved_note(result.1, 0)),
            Fact::CiFailed { infra: true } => result.0.nodes@[0].credit == w.nodes@[0].credit
                && (result.0.nodes@[0].waiting_incident
                    || result.0.nodes@[0].run_failures > w.nodes@[0].run_failures)
                && match w.nodes@[0].effect {
                    None => true,
                    Some(e) => match result.0.nodes@[0].effect {
                        Some(current) => current.seq == e.seq && current.cancelling, None => false,
                    },
                },
            _ => true,
        },
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(storage_ordered);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(accepted); reveal(bounded); reveal(notices_only); reveal(fact_node); reveal(stopped); reveal(incident);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        assert(world_headroom(w));
    }
    kernel_proof! { dependencies::indexable_definition(w); }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    let mut out = Outbox::new();
    apply(&mut w, Event::Fact { node: NodeId(0), fact }, &mut out);
    kernel_proof! {
        fact_preserves_world(before@, w, fact);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out)
}
}

verus! {
/// State-table fact rows / #806: common frame and arithmetic postconditions of
/// the real handlers. The exact node update is specified separately.
#[verifier::opaque]
pub(super) open spec fn fact_frame(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox) -> bool {
    ((before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed))
        && (after.nodes@[i as int].repo == before.nodes@[i as int].repo)
        && (after.nodes@[i as int].footprint == before.nodes@[i as int].footprint)
        && ((settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])))
        && (after.nodes@.len() == before.nodes@.len())
        && (forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j])
        && (after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1)
        && (after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq)
        && (after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start)
        && (after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned)
        && (after.nodes@[i as int].credit.added == before.nodes@[i as int].credit.added)
        && (after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1)
        && (after.next_ticket == before.next_ticket)
        && (after.next_question as int <= before.next_question as int + 1)
        && (after.gates@ == before.gates@)
        && (after.pool_capacity == before.pool_capacity)
        && (after.fail_fast == before.fail_fast)
        && (after.file_sets@ == before.file_sets@)
        && (after.dependencies@ == before.dependencies@)
        && (out_after.len <= out_before.len + 2)
        && (after.nodes@[i as int].last_cmd == before.nodes@[i as int].last_cmd)
}
}

verus! {
/// State-table infrastructure row / #806: derive the common frame separately
/// from the executable branch proof to bound each solver query.
pub(super) proof fn infrastructure_frame(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        after.nodes@[i as int] == fact_node(before.nodes@[i as int], Fact::CiFailed { infra: true }),
        before.nodes@[i as int].phase == Phase::Queued ||
            (before.nodes@[i as int].phase == Phase::Publishing
                && current_item(before.nodes@[i as int]) == Some(Item::Do(EffectKind::Enqueue))),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i ==> after.nodes@[j] == before.nodes@[j],
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast, after.next_ticket == before.next_ticket,
        after.next_question == before.next_question, out_after.len <= out_before.len + 1,
    ensures fact_frame(before, after, i, out_before, out_after),
{
    reveal(fact_node); reveal(fact_frame);
}
}

verus! {
/// State-table Q50: counter/first-retry result before any incident is raised.
#[verifier::opaque]
pub(super) open spec fn infrastructure_retry(n: Node) -> (Node, Option<bool>) {
    if n.run_failures == 0 {
        (phased(Node { run_failures: 1, ..n }, Phase::Publishing), None)
    } else {
        let count = if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 };
        (Node { run_failures: 0, run_incidents: count, ..n }, Some(count >= 2))
    }
}

/// State-table Q50 / #806: compose the actual counter helper and incident helper.
pub(super) proof fn infrastructure_result(before: Node, after: Node, escalation: Option<bool>)
    requires escalation == infrastructure_retry(before).1,
        after == match escalation {
            Some(needs_rob) => incident(infrastructure_retry(before).0, needs_rob),
            None => infrastructure_retry(before).0,
        },
        before.phase == Phase::Queued || (before.phase == Phase::Publishing
            && current_item(before) == Some(Item::Do(EffectKind::Enqueue))),
    ensures after == fact_node(before, Fact::CiFailed { infra: true }),
{
    reveal(infrastructure_retry); reveal(incident); reveal(fact_node);
}
}
