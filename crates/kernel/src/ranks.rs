//! Mathematical ranks for the executable state-table code, #771.
//! These definitions contain no transition relation. Executable rank functions
//! prove their contracts against these mathematical values.
use super::*;
mod effects;
use effects::*;
mod cleanup;
use cleanup::*;
mod node_ranks;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
verus! {
pub open spec fn resumed_phase(r: Resume) -> Phase {
    match r {
        Resume::Making => Phase::Making,
        Resume::Checking => Phase::Checking,
        Resume::Reviewing => Phase::Reviewing,
        Resume::Publishing => Phase::Publishing,
        Resume::Rebasing => Phase::Rebasing,
    }
}

pub open spec fn phase_rank(p: Phase) -> nat
    decreases if p is Ready { 1nat } else { 0nat },
{
    match p {
        Phase::Closed(_) => 0,
        Phase::Stopping(_) => 2,
        Phase::Queued => 4,
        Phase::Publishing => 6,
        Phase::Reviewing => 10,
        Phase::Checking => 14,
        Phase::Rebasing => 18,
        Phase::Making => 22,
        Phase::Ready(r) => phase_rank(resumed_phase(r)) + 2,
        Phase::Planning { .. } => 26,
        Phase::Unplanned => 28,
        Phase::Auditing => 30,
        Phase::Ingesting => 32,
        Phase::AwaitingRob => 3,
    }
}

pub open spec fn continuation_rank(c: Cont) -> nat {
    match c {
        Cont::Ingesting => 32,
        Cont::Auditing => 30,
        Cont::Unplanned => 28,
        Cont::Planning { .. } => 26,
        Cont::Ready(r) => phase_rank(Phase::Ready(r)),
        Cont::Stop(_) | Cont::Nowhere => 2,
    }
}

pub(super) open spec fn items(p: Phase) -> Seq<Item> {
    use EffectKind as K;
    use Item as I;
    match p {
        Phase::Ingesting => seq![I::Start(Role::Ingest), I::Await],
        Phase::Auditing => seq![I::Start(Role::Audit), I::Await],
        Phase::Unplanned => seq![I::Admit],
        Phase::Planning { .. } => seq![I::Start(Role::Plan), I::Await],
        Phase::Ready(_) => seq![I::Slot],
        Phase::Making => seq![I::Do(K::PrepareWorktree), I::Start(Role::Make), I::Await],
        Phase::Checking => seq![I::StartChecks, I::Await],
        Phase::Reviewing => seq![I::Start(Role::Review), I::Await],
        Phase::Publishing => seq![I::Do(K::Push), I::Do(K::PostApproval),
            I::Do(K::OpenPr), I::Do(K::Enqueue)],
        Phase::Queued => seq![I::Merge],
        Phase::Rebasing => seq![I::StartRebase, I::Await],
        Phase::AwaitingRob => seq![I::Answer],
        Phase::Stopping(_) => seq![I::Teardown(K::StopRun), I::Teardown(K::WithdrawJob),
            I::Teardown(K::ClosePr), I::Release, I::Teardown(K::RemoveWorktree)],
        Phase::Closed(_) => seq![],
    }
}

pub(super) open spec fn current_item(n: Node) -> Option<Item> {
    if n.pos < items(n.phase).len() { Some(items(n.phase)[n.pos as int]) } else { None }
}

pub open spec fn holding(p: Phase) -> bool {
    matches!(p, Phase::Making | Phase::Checking | Phase::Reviewing
        | Phase::Publishing | Phase::Queued | Phase::Rebasing)
}

pub(super) open spec fn pause_due(n: Node) -> bool {
    n.paused && holding(n.phase) && !n.waiting_incident && n.effect.is_none()
        && matches!(current_item(n), Some(Item::Do(_) | Item::Start(_)
            | Item::StartChecks | Item::StartRebase))
}

pub closed spec fn pending_rank(n: Node) -> nat {
    if n.phase is Closed { 0 }
    else if n.waiting_incident { if n.ticket.is_some() { 1 } else { 0 } }
    else if n.effect.is_some() { 0 }
    else if pause_due(n) { 3 }
    else { match current_item(n) {
        Some(Item::Start(_) | Item::Slot) => match (n.paused, n.ticket) {
            (false, None) => 2,
            (true, Some(_)) => 1,
            _ => 0,
        },
        _ => 0,
    } }
}

pub(super) open spec fn base_distance(n: Node) -> nat {
    let rank = match n.phase {
        Phase::AwaitingRob => match n.question {
            Some(q) => continuation_rank(q.cont) + 1,
            None => 3,
        },
        p => phase_rank(p),
    };
    let len = items(n.phase).len();
    let pos = if n.pos < len { n.pos as nat } else { len };
    (rank * 64 + 2 * (len - pos)) as nat
}

pub(super) open spec fn effect_distance(n: Node) -> int {
    match n.effect {
        None => 0int,
        Some(e) => if e.cancelling { 1int } else {
            match current_item(n) {
                Some(Item::Do(k)) => if k == e.kind { -1int } else { 1int },
                Some(Item::Teardown(k)) => if k == e.kind { -1int } else { 1int },
                _ => 1int,
            }
        },
    }
}

#[verifier::opaque]
pub(super) open spec fn distance(n: Node) -> nat {
    if n.phase is Closed { 0 }
    else if pause_due(n) {
        phase_rank(Phase::Ready(match n.phase {
            Phase::Making => Resume::Making,
            Phase::Checking => Resume::Checking,
            Phase::Reviewing => Resume::Reviewing,
            Phase::Publishing | Phase::Queued => Resume::Publishing,
            _ => Resume::Rebasing,
        })) * 64 + 10
    } else {
        (base_distance(n) + effect_distance(n)
            + if n.paused && !(n.phase is Stopping) { 8int } else { 0int }
            + if n.waiting_incident { 8int } else { 0int }
            + if n.question.is_some() && n.phase is Stopping { 4int } else { 0int }) as nat
    }
}

/// #771: public view of the executable node distance.
pub closed spec fn node_distance(n: Node) -> nat { distance(n) }

/// #771: local moves left before this node blocks, independent of scheduling.
/// Stopping has five agenda entries and closure, hence at most six moves.
pub(super) open spec fn internal_height(n: Node) -> nat {
    if n.phase is Closed { 0 }
    else if n.waiting_incident { if n.ticket.is_some() { 1 } else { 0 } }
    else if n.effect.is_some() { 0 }
    else { match current_item(n) {
        None => match n.phase {
            Phase::Publishing => 1,
            Phase::Stopping(_) => if n.question.is_none() { 1 } else { 0 },
            _ => 0,
        },
        Some(Item::Admit) => if n.paused { 0 } else { 3 },
        Some(Item::Slot) => if n.paused {
            if n.ticket.is_some() { 1 } else { 0 }
        } else { if n.ticket.is_some() { 3 } else { 4 } },
        Some(Item::Start(_)) => if n.paused {
            if holding(n.phase) || n.ticket.is_some() { 1 } else { 0 }
        } else { if n.ticket.is_some() { 1 } else { 2 } },
        Some(Item::Do(_) | Item::StartChecks | Item::StartRebase) => 1,
        Some(Item::Release) => (6 - n.pos) as nat,
        Some(Item::Teardown(_)) => if n.question.is_some() { 0 } else { (6 - n.pos) as nat },
        _ => 0,
    } }
}

/// #771: at most one phase change remains within a node's settle chain.
pub(super) open spec fn generation_capacity(n: Node) -> nat {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() { 0 }
    else if pause_due(n) { 1 }
    else { match current_item(n) {
        Some(Item::Admit | Item::Slot) => if n.paused { 0 } else { 1 },
        None => if n.phase == Phase::Publishing { 1 } else { 0 },
        _ => 0,
    } }
}

/// #771: emitting an effect blocks the node for the rest of settle.
pub(super) open spec fn effect_capacity(n: Node) -> nat {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() { 0 } else { 1 }
}

/// Q39 / #771: Ready can take a slot ticket followed by a pool ticket.
pub(super) open spec fn ticket_capacity(n: Node) -> nat {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() || n.paused { 0 }
    else { match current_item(n) {
        Some(Item::Slot) => if n.ticket.is_some() { 1 } else { 2 },
        Some(Item::Admit) => 1,
        Some(Item::Start(_)) => if n.ticket.is_some() { 0 } else { 1 },
        _ => 0,
    } }
}

pub(super) open spec fn merge_capacity(n: Node) -> nat {
    if n.credit.merge_credited || n.phase is Closed { 0 } else { 1 }
}

/// #771: these are mathematical potentials, not additional machine counters.
pub(super) proof fn local_capacities(n: Node)
    ensures internal_height(n) <= 6, generation_capacity(n) <= 1,
        effect_capacity(n) <= 1, ticket_capacity(n) <= 2, merge_capacity(n) <= 1,
{
}


}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: the subset of consistency needed to bound settle's scratch scans.
/// Cleanup releases the pool at its first step and its slot at Release.
pub(super) open spec fn settle_shape(n: Node) -> bool {
    (n.phase is Closed ==> !n.pool && !n.holds)
        && (n.phase is Stopping && n.pos >= 1 ==> !n.pool)
        && (n.phase is Stopping && n.pos >= 4 ==> !n.holds)
}

/// Q30/Q39 / #806: expose the exact mathematical ranks to the independently
/// gated real slot transition, without trusting an opaque rank identity.
pub(super) proof fn rank_definitions(n: Node)
    ensures node_distance(n) == distance(n), pending_rank(n) == {

    if n.phase is Closed { 0nat }
    else if n.waiting_incident { if n.ticket.is_some() { 1 } else { 0 } }
    else if n.effect.is_some() { 0 }
    else if pause_due(n) { 3 }
    else { match current_item(n) {
        Some(Item::Start(_) | Item::Slot) => match (n.paused, n.ticket) {
            (false, None) => 2,
            (true, Some(_)) => 1,
            _ => 0,
        },
        _ => 0,
    } }
    },
{
}

/// #771: exact enabling condition of the executable local dispatcher.
/// This observes the agenda and scheduler decisions; it defines no replacement
/// transition relation and is checked against every production case body.
pub(super) open spec fn advance_enabled(n: Node, context: AdvanceContext) -> bool {
    if n.phase is Closed { false }
    else if n.waiting_incident { n.ticket.is_some() }
    else if n.effect.is_some() { false }
    else { match current_item(n) {
        None => n.phase == Phase::Publishing || (n.phase is Stopping && n.question.is_none()),
        Some(Item::Do(_) | Item::StartChecks | Item::StartRebase) =>
            !n.paused || holding(n.phase) || n.ticket.is_some(),
        Some(Item::Start(_)) => if n.paused { holding(n.phase) || n.ticket.is_some() }
            else { n.ticket.is_none() || context.pool },
        Some(Item::Admit) => !n.paused && context.admit,
        Some(Item::Slot) => n.phase is Ready && if n.paused { n.ticket.is_some() }
            else { n.ticket.is_none() || context.slot },
        Some(Item::Release) => true,
        Some(Item::Teardown(_)) => n.question.is_none(),
        Some(Item::Await | Item::Merge | Item::Answer) => false,
    } }
}

pub(super) open spec fn advance_pre(n: Node, ticket: u64, out_len: usize) -> bool {
    out_len as int + 2 <= OUTBOX as int
        && n.generation as int + generation_capacity(n) <= u64::MAX as int
        && n.next_seq as int + effect_capacity(n) <= u64::MAX as int
        && ticket as int + ticket_capacity(n) <= u64::MAX as int
        && n.credit.earned as int + merge_capacity(n) <= u64::MAX as int
}

/// #771: bounds proved for the actual local transition. This is a postcondition,
/// not an alternative transition relation.
#[verifier::opaque]
pub(super) open spec fn advance_post(before: Node, after: Node,
    ticket_before: u64, ticket_after: u64, changed: bool) -> bool {
    (changed ==> internal_height(after) < internal_height(before))
    && after.last_cmd == before.last_cmd
    && after.repo == before.repo && after.footprint == before.footprint
    && (settle_shape(before) ==> settle_shape(after))
    && (before.phase is Closed ==> after.phase is Closed)
    && distance(after) <= distance(before)
    && (changed ==> 4 * distance(after) + pending_rank(after)
        < 4 * distance(before) + pending_rank(before))
    && after.generation as int + generation_capacity(after)
        <= before.generation as int + generation_capacity(before)
    && after.next_seq as int + effect_capacity(after)
        <= before.next_seq as int + effect_capacity(before)
    && ticket_after as int + ticket_capacity(after)
        <= ticket_before as int + ticket_capacity(before)
    && after.credit.earned as int + merge_capacity(after)
        <= before.credit.earned as int + merge_capacity(before)
    && after.credit.start == before.credit.start
    && after.credit.added == before.credit.added
    && after.credit.spent == before.credit.spent
    && (!changed ==> after == before && ticket_after == ticket_before)
}
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_node(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(out).len),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
            (old(node).phase == Phase::Ready(Resume::Making) && old(node).pos == 0 && !old(node).waiting_incident && old(node).effect.is_none()) ==> {
                *final(node) == retirement_gate::slot_node(*old(node), *old(next_ticket), context.slot)
                    && *final(next_ticket) == retirement_gate::slot_ticket(*old(node), *old(next_ticket))
                    && *final(out) == *old(out)
                    && result == retirement_gate::slot_changed(*old(node), context.slot)
            },
    ] {
        let n = *node;
        if n.phase.is_closed() || n.waiting_incident || n.effect.is_some() {
            return advance_blocked(node, next_ticket, context, out);
        }
        let Some(item) = n.current() else {
            return advance_finished(node, next_ticket, context, out);
        };
        match item {
            Item::Do(kind) => advance_do(node, next_ticket, context, out, kind),
            Item::Start(role) => advance_start(node, next_ticket, context, out, role),
            Item::StartChecks => advance_checks(node, next_ticket, context, out),
            Item::StartRebase => advance_rebase(node, next_ticket, context, out),
            Item::Await | Item::Merge | Item::Answer => advance_idle(node, next_ticket, context, out),
            Item::Admit => advance_admit(node, next_ticket, context, out),
            Item::Slot => advance_slot(node, next_ticket, context, out),
            Item::Release => advance_release(node, next_ticket, context, out),
            Item::Teardown(kind) => advance_teardown(node, next_ticket, context, out, kind),
        }
    }
}
