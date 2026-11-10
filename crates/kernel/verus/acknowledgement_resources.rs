use super::*;

verus! {
#[verifier::opaque]
pub(super) open spec fn confirmed_rules_transfer(b: Node, n: Node) -> bool {
    forall|w: World| #[trigger] retirement::node_rules(w, b) ==> retirement::node_rules(w, n)
}

pub(super) open spec fn confirmed_teardown_input(b: Node, kind: EffectKind) -> bool {
    b.phase is Stopping && transition_safety::teardown_kind(kind)
        && current_item(b) == Some(Item::Teardown(kind))
}

pub(super) open spec fn confirmed_step_ready(b: Node) -> bool {
    b.phase is Stopping && b.effect.is_none() && b.question.is_none()
        && match current_item(b) {
            Some(Item::Teardown(EffectKind::StopRun)) => b.run.is_none() && !b.pool,
            Some(Item::Teardown(EffectKind::WithdrawJob)) => b.job.is_none(),
            Some(Item::Teardown(EffectKind::ClosePr)) => !b.pr_open,
            Some(Item::Teardown(EffectKind::RemoveWorktree)) => !b.worktree,
            _ => false,
        }
}

#[verifier::opaque]
pub(super) open spec fn confirmed_clear_frame(b: Node, n: Node) -> bool {
    (n.effect.is_none())
        && (n.phase == b.phase)
        && (n.pos == b.pos)
        && (n.holds == b.holds)
        && (n.pool == b.pool)
        && (n.run == b.run)
        && (n.job == b.job)
        && (n.pr_open == b.pr_open)
        && (n.worktree == b.worktree)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.fx_failures == b.fx_failures)
        && (n.fx_incidents == b.fx_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (confirmed_rules_transfer(b, n))
}

#[verifier::opaque]
pub(super) open spec fn confirmed_cancel_frame(b: Node, n: Node) -> bool {
    (n.effect == b.effect)
        && (n.phase == b.phase)
        && (n.pos == b.pos)
        && (n.holds == b.holds)
        && (n.pool == b.pool)
        && (n.run == b.run)
        && (n.job == b.job)
        && (n.pr_open == b.pr_open)
        && (n.worktree == b.worktree)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.fx_failures == b.fx_failures)
        && (n.fx_incidents == b.fx_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (confirmed_rules_transfer(b, n))
}

#[verifier::opaque]
pub(super) open spec fn confirmed_resources_frame(b: Node, n: Node, kind: EffectKind) -> bool {
    (n.phase == b.phase)
        && (n.pos == b.pos)
        && (n.holds == b.holds)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (n.effect == b.effect)
        && (n.fx_failures == 0)
        && (n.fx_incidents == 0)
        && (n.run == (if kind == EffectKind::StopRun { None } else { b.run }))
        && (n.pool == (if kind == EffectKind::StopRun { false } else { b.pool }))
        && (n.job == (if kind == EffectKind::WithdrawJob { None } else { b.job }))
        && (n.pr_open == (if kind == EffectKind::ClosePr { false } else { b.pr_open }))
        && (n.worktree == (if kind == EffectKind::RemoveWorktree { false } else { b.worktree }))
        && ((confirmed_teardown_input(b, kind) && b.effect.is_none() && b.question.is_none()) ==> confirmed_rules_transfer(b, n))
}

#[verifier::opaque]
pub(super) open spec fn confirmed_step_frame(b: Node, n: Node) -> bool {
    (n.pos == b.pos + 1)
        && (n.effect == b.effect)
        && (n.phase == b.phase)
        && (n.holds == b.holds)
        && (n.pool == b.pool)
        && (n.run == b.run)
        && (n.job == b.job)
        && (n.pr_open == b.pr_open)
        && (n.worktree == b.worktree)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.fx_failures == b.fx_failures)
        && (n.fx_incidents == b.fx_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (confirmed_step_ready(b) ==> confirmed_rules_transfer(b, n))
}

#[verifier::opaque]
pub(super) open spec fn confirmed_agenda_frame(b: Node, n: Node, kind: EffectKind) -> bool {
    (confirmed_teardown_input(b, kind) ==> ((n.pos == b.pos + 1)
        && (n.effect == b.effect)
        && (n.phase == b.phase)
        && (n.holds == b.holds)
        && (n.pool == b.pool)
        && (n.run == b.run)
        && (n.job == b.job)
        && (n.pr_open == b.pr_open)
        && (n.worktree == b.worktree)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.fx_failures == b.fx_failures)
        && (n.fx_incidents == b.fx_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)))
        && ((confirmed_teardown_input(b, kind) && confirmed_step_ready(b)) ==> confirmed_rules_transfer(b, n))
}

#[verifier::opaque]
pub(super) open spec fn confirmed_value_frame(b: Node, n: Node, kind: EffectKind) -> bool {
    (confirmed_teardown_input(b, kind) ==> ((n.phase == b.phase)
        && (n.holds == b.holds)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (n.pos == b.pos + 1)
        && (n.effect == b.effect)
        && (n.fx_failures == 0)
        && (n.fx_incidents == 0)
        && (n.run == (if kind == EffectKind::StopRun { None } else { b.run }))
        && (n.pool == (if kind == EffectKind::StopRun { false } else { b.pool }))
        && (n.job == (if kind == EffectKind::WithdrawJob { None } else { b.job }))
        && (n.pr_open == (if kind == EffectKind::ClosePr { false } else { b.pr_open }))
        && (n.worktree == (if kind == EffectKind::RemoveWorktree { false } else { b.worktree }))))
        && ((confirmed_teardown_input(b, kind) && b.effect.is_none() && b.question.is_none()) ==> confirmed_rules_transfer(b, n))
}

#[verifier::opaque]
pub(super) open spec fn confirmed_ack_frame(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64) -> bool {
    ((ack == Ack::Confirmed && e.cancelling) ==> ((n.effect.is_none())
        && (n.phase == b.phase)
        && (n.pos == b.pos)
        && (n.holds == b.holds)
        && (n.pool == b.pool)
        && (n.run == b.run)
        && (n.job == b.job)
        && (n.pr_open == b.pr_open)
        && (n.worktree == b.worktree)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.fx_failures == b.fx_failures)
        && (n.fx_incidents == b.fx_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)))
        && ((ack == Ack::Confirmed && !e.cancelling && confirmed_teardown_input(b, e.kind)) ==> ((n.phase == b.phase)
        && (n.holds == b.holds)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (n.pos == b.pos + 1)
        && (n.effect.is_none())
        && (n.fx_failures == 0)
        && (n.fx_incidents == 0)
        && (n.run == (if e.kind == EffectKind::StopRun { None } else { b.run }))
        && (n.pool == (if e.kind == EffectKind::StopRun { false } else { b.pool }))
        && (n.job == (if e.kind == EffectKind::WithdrawJob { None } else { b.job }))
        && (n.pr_open == (if e.kind == EffectKind::ClosePr { false } else { b.pr_open }))
        && (n.worktree == (if e.kind == EffectKind::RemoveWorktree { false } else { b.worktree }))))
        && (((ack == Ack::Confirmed && e.cancelling) || (ack == Ack::Confirmed && !e.cancelling && confirmed_teardown_input(b, e.kind))) ==> n.ticket == b.ticket && n.generation == b.generation && nqa == nq)
        && ((((ack == Ack::Confirmed && e.cancelling) || (ack == Ack::Confirmed && !e.cancelling && confirmed_teardown_input(b, e.kind))) && b.effect == Some(e)) ==> confirmed_rules_transfer(b, n))
}

pub(super) proof fn confirmed_clear_node_rules(w: World, b: Node)
    requires retirement::node_rules(w, b),
    ensures retirement::node_rules(w, Node { effect: None, ..b }),
{
    reveal(retirement::node_rules);
}

pub(super) proof fn confirmed_clear_rows(b: Node)
    ensures confirmed_clear_frame(b, Node { effect: None, ..b }),
{
    reveal(confirmed_clear_frame);
    reveal(confirmed_rules_transfer);
    assert forall|w: World| #[trigger] retirement::node_rules(w, b) implies retirement::node_rules(w, Node { effect: None, ..b }) by {
        confirmed_clear_node_rules(w, b);
    };
}

pub(super) proof fn confirmed_cancel_rows(b: Node)
    ensures confirmed_cancel_frame(b, b),
{
    reveal(confirmed_cancel_frame);
    reveal(confirmed_rules_transfer);
}

pub(super) proof fn confirmed_resources_node_rules(w: World, b: Node, kind: EffectKind)
    requires retirement::node_rules(w, b), confirmed_teardown_input(b, kind) && b.effect.is_none() && b.question.is_none(),
    ensures retirement::node_rules(w, Node { fx_failures: 0, fx_incidents: 0, run: if kind == EffectKind::StopRun { None } else { b.run }, pool: if kind == EffectKind::StopRun { false } else { b.pool }, job: if kind == EffectKind::WithdrawJob { None } else { b.job }, pr_open: if kind == EffectKind::ClosePr { false } else { b.pr_open }, worktree: if kind == EffectKind::RemoveWorktree { false } else { b.worktree }, ..b }),
{
    reveal(retirement::node_rules);
    if kind == EffectKind::StopRun { assert(b.pos == 0); }
    else if kind == EffectKind::WithdrawJob { assert(b.pos == 1); }
    else if kind == EffectKind::ClosePr { assert(b.pos == 2); }
    else { assert(kind == EffectKind::RemoveWorktree); assert(b.pos == 4); }
}

pub(super) proof fn confirmed_resources_rows(b: Node, kind: EffectKind)
    ensures confirmed_resources_frame(b, Node { fx_failures: 0, fx_incidents: 0, run: if kind == EffectKind::StopRun { None } else { b.run }, pool: if kind == EffectKind::StopRun { false } else { b.pool }, job: if kind == EffectKind::WithdrawJob { None } else { b.job }, pr_open: if kind == EffectKind::ClosePr { false } else { b.pr_open }, worktree: if kind == EffectKind::RemoveWorktree { false } else { b.worktree }, ..b }, kind),
{
    reveal(confirmed_resources_frame);
    if confirmed_teardown_input(b, kind) && b.effect.is_none() && b.question.is_none() {
        reveal(confirmed_rules_transfer);
        assert forall|w: World| #[trigger] retirement::node_rules(w, b) implies retirement::node_rules(w, Node { fx_failures: 0, fx_incidents: 0, run: if kind == EffectKind::StopRun { None } else { b.run }, pool: if kind == EffectKind::StopRun { false } else { b.pool }, job: if kind == EffectKind::WithdrawJob { None } else { b.job }, pr_open: if kind == EffectKind::ClosePr { false } else { b.pr_open }, worktree: if kind == EffectKind::RemoveWorktree { false } else { b.worktree }, ..b }) by {
            confirmed_resources_node_rules(w, b, kind);
        };
    }
}

pub(super) proof fn confirmed_step_node_rules(w: World, b: Node)
    requires retirement::node_rules(w, b), confirmed_step_ready(b), b.pos < u8::MAX,
    ensures retirement::node_rules(w, Node { pos: (b.pos + 1) as u8, ..b }),
{
    reveal(retirement::node_rules);
    match current_item(b) {
        Some(Item::Teardown(EffectKind::StopRun)) => { assert(b.pos == 0); },
        Some(Item::Teardown(EffectKind::WithdrawJob)) => { assert(b.pos == 1); },
        Some(Item::Teardown(EffectKind::ClosePr)) => { assert(b.pos == 2); },
        _ => { assert(b.pos == 4); },
    }

}

pub(super) proof fn confirmed_agenda_teardown_rows(b: Node)
    requires b.pos < u8::MAX,
    ensures confirmed_step_frame(b, Node { pos: (b.pos + 1) as u8, ..b }),
{
    reveal(confirmed_step_frame);
    if confirmed_step_ready(b) {
        reveal(confirmed_rules_transfer);
        assert forall|w: World| #[trigger] retirement::node_rules(w, b) implies retirement::node_rules(w, Node { pos: (b.pos + 1) as u8, ..b }) by {
            confirmed_step_node_rules(w, b);
        };
    }
}

pub(super) proof fn confirmed_agenda_step_rows(b: Node, n: Node, kind: EffectKind)
    requires confirmed_teardown_input(b, kind), confirmed_step_frame(b, n),
    ensures confirmed_agenda_frame(b, n, kind),
{
    reveal(confirmed_step_frame);
    reveal(confirmed_agenda_frame);
}

pub(super) proof fn confirmed_agenda_other_rows(b: Node, n: Node, kind: EffectKind)
    requires !confirmed_teardown_input(b, kind),
    ensures confirmed_agenda_frame(b, n, kind),
{
    reveal(confirmed_agenda_frame);
}

pub(super) proof fn confirmed_value_rows(b: Node, r: Node, n: Node, kind: EffectKind)
    requires confirmed_resources_frame(b, r, kind), confirmed_agenda_frame(r, n, kind),
    ensures confirmed_value_frame(b, n, kind),
{
    reveal(confirmed_resources_frame);
    reveal(confirmed_agenda_frame);
    reveal(confirmed_value_frame);
    if confirmed_teardown_input(b, kind) && b.effect.is_none() && b.question.is_none() {
        assert(confirmed_step_ready(r));
        reveal(confirmed_rules_transfer);
        assert forall|w: World| #[trigger] retirement::node_rules(w, b) implies retirement::node_rules(w, n) by {
            assert(retirement::node_rules(w, r));
        };
    }
}

pub(super) proof fn confirmed_live_question(w: World, b: Node, e: InFlight)
    requires retirement::node_rules(w, b), b.effect == Some(e), !e.cancelling,
    ensures b.question.is_none(),
{
    reveal(retirement::node_rules);
}

pub(super) proof fn confirmed_cancel_forward(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64)
    requires e.cancelling, confirmed_cancel_frame(Node { effect: None, ..b }, n), nqa == nq,
    ensures confirmed_ack_frame(b, n, e, ack, nq, nqa),
{
    confirmed_clear_rows(b);
    reveal(confirmed_clear_frame);
    reveal(confirmed_cancel_frame);
    reveal(confirmed_ack_frame);
    reveal(confirmed_rules_transfer);
    assert forall|w: World| #[trigger] retirement::node_rules(w, b) implies retirement::node_rules(w, n) by {
        assert(retirement::node_rules(w, Node { effect: None, ..b }));
    };
}

pub(super) proof fn confirmed_value_forward(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64)
    requires !e.cancelling, ack == Ack::Confirmed, confirmed_value_frame(Node { effect: None, ..b }, n, e.kind), nqa == nq,
    ensures confirmed_ack_frame(b, n, e, ack, nq, nqa),
{
    confirmed_clear_rows(b);
    reveal(confirmed_clear_frame);
    reveal(confirmed_value_frame);
    reveal(confirmed_ack_frame);
    if confirmed_teardown_input(b, e.kind) && b.effect == Some(e) {
        reveal(confirmed_rules_transfer);
        assert forall|w: World| #[trigger] retirement::node_rules(w, b) implies retirement::node_rules(w, n) by {
            confirmed_live_question(w, b, e);
            assert(retirement::node_rules(w, Node { effect: None, ..b }));
        };
    }
}

pub(super) proof fn confirmed_irrelevant_rows(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64)
    requires ack != Ack::Confirmed,
    ensures confirmed_ack_frame(b, n, e, ack, nq, nqa),
{
    reveal(confirmed_ack_frame);
}

pub(super) proof fn confirmed_ack_rows(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64)
    requires confirmed_ack_frame(b, n, e, ack, nq, nqa),
    ensures (ack == Ack::Confirmed && e.cancelling) ==> ((n.effect.is_none())
        && (n.phase == b.phase)
        && (n.pos == b.pos)
        && (n.holds == b.holds)
        && (n.pool == b.pool)
        && (n.run == b.run)
        && (n.job == b.job)
        && (n.pr_open == b.pr_open)
        && (n.worktree == b.worktree)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.fx_failures == b.fx_failures)
        && (n.fx_incidents == b.fx_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)),
        (ack == Ack::Confirmed && !e.cancelling && confirmed_teardown_input(b, e.kind)) ==> ((n.phase == b.phase)
        && (n.holds == b.holds)
        && (n.next_seq == b.next_seq)
        && (n.question == b.question)
        && (n.run_failures == b.run_failures)
        && (n.run_incidents == b.run_incidents)
        && (n.ticket == b.ticket)
        && (n.generation == b.generation)
        && (n.pos == b.pos + 1)
        && (n.effect.is_none())
        && (n.fx_failures == 0)
        && (n.fx_incidents == 0)
        && (n.run == (if e.kind == EffectKind::StopRun { None } else { b.run }))
        && (n.pool == (if e.kind == EffectKind::StopRun { false } else { b.pool }))
        && (n.job == (if e.kind == EffectKind::WithdrawJob { None } else { b.job }))
        && (n.pr_open == (if e.kind == EffectKind::ClosePr { false } else { b.pr_open }))
        && (n.worktree == (if e.kind == EffectKind::RemoveWorktree { false } else { b.worktree }))),
        ((ack == Ack::Confirmed && e.cancelling) || (ack == Ack::Confirmed && !e.cancelling && confirmed_teardown_input(b, e.kind))) ==> n.ticket == b.ticket && n.generation == b.generation && nqa == nq,
        (((ack == Ack::Confirmed && e.cancelling) || (ack == Ack::Confirmed && !e.cancelling && confirmed_teardown_input(b, e.kind))) && b.effect == Some(e)) ==> confirmed_rules_transfer(b, n),
{
    reveal(confirmed_ack_frame);
}

pub(super) proof fn confirmed_node_rules(w: World, b: Node, n: Node)
    requires retirement::node_rules(w, b), confirmed_rules_transfer(b, n),
    ensures retirement::node_rules(w, n),
{
    reveal(confirmed_rules_transfer);
}
}
