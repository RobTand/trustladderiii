//! K5 G4: the Ack family arm of the frozen G9 node-safety surface, and the
//! producer lemmas Compose's G20 adoption / body-policy theorems consume.
use super::*;

verus! {
/// G9 Ack arm (frozen signature). on_ack's forwarded exact post plus the
/// original-input premise node_rules(before, b) give the per-node contract.
pub(super) proof fn ack_node_safety(before: World, after: World, event: Event, i: int,
    ob: Outbox, oa: Outbox)
    requires transition_safety::event_target(event) == Some(i), 0 <= i < before.nodes@.len(),
        retirement::node_rules(before, before.nodes@[i]),
        match event {
            Event::Ack { seq, ack, .. } => acknowledgements::on_ack_post(before, after, i, seq, ack, ob, oa),
            _ => false,
        },
    ensures transition_safety::node_safety(before.nodes@[i], after.nodes@[i], before.fail_fast,
        event, i, ob, oa),
{
    reveal(transition_safety::node_safety);
    reveal(acknowledgements::on_ack_post);
    reveal(acknowledgements::ack_live_post);
    reveal(retirement::node_rules);
    let b = before.nodes@[i];
    if let Event::Ack { seq, ack, .. } = event {
        if let Some(e) = b.effect {
            if !acknowledgements::ack_stutters(b, seq, ack) && !e.cancelling
                && ack == Ack::BodyPolicyRefused && e.kind == EffectKind::OpenPr {
                allocation_stopping_lemmas::stopped_cases(
                    Node { review_stop: Some(ReviewStop::PublicationBody), effect: None, ..b },
                    Outcome::Stopped);
            }
        }
    }
}

/// G20 producer (H57.2 / H58.2): a confirmed PrepareWorktree at its agenda item
/// adopts the proposed head once, enters Checking without a maker run or credit
/// charge, and a later prepare keeps the existing head and phase.
pub(super) proof fn adoption_facts(w: World, b: Node, n: Node)
    requires retirement::node_rules(w, b),
        current_item(b) == Some(Item::Do(EffectKind::PrepareWorktree)),
        n == acknowledgements::agenda_spec(acknowledgements::resources_spec(
            Node { effect: None, ..b }, EffectKind::PrepareWorktree), EffectKind::PrepareWorktree),
    ensures n.credit == b.credit, n.attempt == b.attempt, n.round == b.round,
        n.run.is_none(), !n.pool,
        b.adopted ==> n.adopted && n.head == b.head && n.phase == b.phase,
        !b.adopted && b.proposed.is_some() ==> n.adopted && n.head == b.proposed
            && n.checked.is_none() && n.phase == Phase::Checking,
{
    reveal(retirement::node_rules);
}

/// G20 producer (H29.2): outside a live OpenPr effect, a body-policy refusal has
/// exactly the Permanent result: same node, same appended outputs, same question id.
pub(super) proof fn bpr_equiv(b: Node, e: InFlight, nq: u64, i: int, ob: Outbox,
    n1: Node, nqa1: u64, oa1: Outbox, n2: Node, nqa2: u64, oa2: Outbox)
    requires !e.cancelling, e.kind != EffectKind::OpenPr,
        acknowledgements::ack_live_post(b, n1, e, Ack::BodyPolicyRefused, nq, nqa1, i, ob, oa1),
        acknowledgements::ack_live_post(b, n2, e, Ack::Permanent, nq, nqa2, i, ob, oa2),
    ensures n1 == n2, nqa1 == nqa2, oa1.len == oa2.len, oa1.items@ == oa2.items@,
{
    reveal(acknowledgements::ack_live_post);
}

/// G20 producer (H29.2): an OpenPr body-policy refusal stops for TL3 review and
/// appends nothing: it never asks Rob or raises an incident.
pub(super) proof fn bpr_open_pr_quiet(b: Node, n: Node, e: InFlight, nq: u64, nqa: u64, i: int,
    ob: Outbox, oa: Outbox)
    requires !e.cancelling, e.kind == EffectKind::OpenPr,
        acknowledgements::ack_live_post(b, n, e, Ack::BodyPolicyRefused, nq, nqa, i, ob, oa),
    ensures oa == ob, nqa == nq, n.review_stop == Some(ReviewStop::PublicationBody),
{
    reveal(acknowledgements::ack_live_post);
    allocation_stopping_lemmas::stopped_cases(
        Node { review_stop: Some(ReviewStop::PublicationBody), effect: None, ..b }, Outcome::Stopped);
}

pub(super) proof fn adoption_ack_result(before: World, after: World, i: int, seq: u64,
    ob: Outbox, oa: Outbox)
    requires 0 <= i < before.nodes@.len(), i < after.nodes@.len(),
        retirement::node_rules(before, before.nodes@[i]),
        current_item(before.nodes@[i]) == Some(Item::Do(EffectKind::PrepareWorktree)),
        match before.nodes@[i].effect {
            Some(e) => e.seq == seq && !e.cancelling && e.kind == EffectKind::PrepareWorktree,
            None => false,
        },
        acknowledgements::on_ack_post(before, after, i, seq, Ack::Confirmed, ob, oa),
    ensures after.nodes@[i].credit == before.nodes@[i].credit,
        after.nodes@[i].attempt == before.nodes@[i].attempt,
        after.nodes@[i].round == before.nodes@[i].round,
        after.nodes@[i].run.is_none(), !after.nodes@[i].pool,
        oa != ob, after.next_question == before.next_question,
        before.nodes@[i].adopted ==> after.nodes@[i].adopted
            && after.nodes@[i].head == before.nodes@[i].head
            && after.nodes@[i].phase == before.nodes@[i].phase,
        !before.nodes@[i].adopted && before.nodes@[i].proposed.is_some()
            ==> after.nodes@[i].adopted && after.nodes@[i].head == before.nodes@[i].proposed
                && after.nodes@[i].checked.is_none() && after.nodes@[i].phase == Phase::Checking,
{
    reveal(acknowledgements::on_ack_post);
    reveal(acknowledgements::ack_live_post);
    adoption_facts(before, before.nodes@[i], after.nodes@[i]);
}

pub(super) proof fn bpr_ack_result_equiv(before: World, refused: World, permanent: World,
    i: int, seq: u64, ob: Outbox, refused_out: Outbox, permanent_out: Outbox)
    requires 0 <= i < before.nodes@.len(), i < refused.nodes@.len(), i < permanent.nodes@.len(),
        acknowledgements::on_ack_post(before, refused, i, seq, Ack::BodyPolicyRefused, ob, refused_out),
        acknowledgements::on_ack_post(before, permanent, i, seq, Ack::Permanent, ob, permanent_out),
        match before.nodes@[i].effect {
            Some(e) => !(e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr),
            None => true,
        },
    ensures refused.nodes@[i] == permanent.nodes@[i],
        refused.next_question == permanent.next_question,
        refused_out.len == permanent_out.len, refused_out.items@ == permanent_out.items@,
{
    reveal(acknowledgements::on_ack_post);
    reveal(acknowledgements::ack_live_post);
    if let Some(e) = before.nodes@[i].effect {
        if e.seq == seq && !e.cancelling {
            bpr_equiv(before.nodes@[i], e, before.next_question, i, ob,
                refused.nodes@[i], refused.next_question, refused_out,
                permanent.nodes@[i], permanent.next_question, permanent_out);
        }
    }
}

}
