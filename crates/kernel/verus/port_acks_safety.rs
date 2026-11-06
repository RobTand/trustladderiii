//! #806: acknowledgement frames, complete checker preservation and Q54.
use super::port_acks_specs::*;
use super::retirement::{accepted, bounded, domain, holders, node_rules};
use super::*;

verus! {
/// State-table checker context: fresh questions cannot invalidate old question IDs.
pub(super) proof fn question_context(b: World, a: World, n: Node)
    requires b.gates@ == a.gates@, b.file_sets@ == a.file_sets@,
        b.next_ticket == a.next_ticket, b.next_question <= a.next_question, node_rules(b, n),
    ensures node_rules(a, n),
{
    reveal(node_rules);
    footprint::storage_frame(b, a, n.footprint, n.footprint);
}

/// State-table cancelled/stale delivery: clearing an in-flight key preserves every checker rule.
pub(super) proof fn clear_rules(w: World, n: Node)
    requires node_rules(w, n),
    ensures node_rules(w, Node { effect: None, ..n }),
{ reveal(node_rules); }

/// State-table effect matching: a live delivery has no owner question or incident wait.
pub(super) proof fn delivery_input(w: World, n: Node, e: InFlight)
    requires node_rules(w, n), n.effect == Some(e), !e.cancelling,
    ensures !(n.phase is Closed), n.question.is_none(), !n.waiting_incident,
        e.kind == EffectKind::OpenPr ==> n.pr_open,
        if matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            current_item(n) == Some(Item::Await)
        } else { current_item(n) == Some(Item::Do(e.kind)) || current_item(n) == Some(Item::Teardown(e.kind)) },
{ reveal(node_rules); }

/// State-table Confirmed resource cleanup: the real effect key selects its resource.
pub(super) proof fn confirmed_cleanup_rules(w: World, n: Node, k: EffectKind)
    requires node_rules(w, n), n.effect.is_none(),
        if matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            current_item(n) == Some(Item::Await)
        } else { current_item(n) == Some(Item::Do(k)) || current_item(n) == Some(Item::Teardown(k)) },
    ensures node_rules(w, confirmed_cleanup(n, k)),
{
    reveal(node_rules); reveal(confirmed_cleanup);
    match k {
        EffectKind::StopRun => {}, EffectKind::WithdrawJob => {}, EffectKind::ClosePr => {},
        EffectKind::RemoveWorktree => {}, _ => {},
    }
}

/// State-table Confirmed agenda: consuming a real delivery preserves the complete checker.
pub(super) proof fn confirmed_agenda_rules(w: World, n: Node, k: EffectKind)
    requires node_rules(w, n), n.effect.is_none(), n.question.is_none(), !n.waiting_incident,
        if matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            current_item(n) == Some(Item::Await)
        } else { current_item(n) == Some(Item::Do(k)) || current_item(n) == Some(Item::Teardown(k)) },
        k == EffectKind::OpenPr ==> n.pr_open,
        k == EffectKind::StopRun ==> n.run.is_none() && !n.pool,
        k == EffectKind::WithdrawJob ==> n.job.is_none(),
        k == EffectKind::ClosePr ==> !n.pr_open,
        k == EffectKind::RemoveWorktree ==> !n.worktree,
        node_headroom(n),
    ensures node_rules(w, confirmed_agenda_node(n, k)),
{
    reveal(node_rules); reveal(confirmed_agenda_node);
    match n.phase {
        Phase::Closed(_) => {}, Phase::Stopping(_) => {},
        Phase::Ingesting => {}, Phase::Auditing => {}, Phase::Unplanned => {},
        Phase::Planning { .. } => {}, Phase::Ready(_) => {}, Phase::Making => {},
        Phase::Checking => {}, Phase::Reviewing => {}, Phase::Publishing => {},
        Phase::Queued => {}, Phase::Rebasing => {}, Phase::AwaitingRob => {},
    }
}

/// State-table Confirmed frame: reset counters, release target, consume matching agenda only.
pub(super) proof fn confirmed_rules(w: World, n: Node, q: u64, seq: u64)
    requires node_rules(w, n), node_headroom(n),
    ensures node_rules(w, ack_node(n, q, seq, Ack::Confirmed)),
{
    reveal(ack_node); reveal(confirmed_node); reveal(confirmed_cleanup);
    if let Some(e) = n.effect {
        if e.seq == seq {
            clear_rules(w, n);
            if !e.cancelling {
                delivery_input(w, n, e);
                let cleared = Node { effect: None, ..n };
                confirmed_cleanup_rules(w, cleared, e.kind);
                confirmed_agenda_rules(w, confirmed_cleanup(cleared, e.kind), e.kind);
            }
        }
    }
}

/// State-table retry counters: the second failure abandons only the matching operation.
pub(super) proof fn retry_rules(w: World, n: Node, q: u64, seq: u64)
    requires node_rules(w, n), node_headroom(n),
    ensures node_rules(w, ack_node(n, q, seq, Ack::Retryable)),
{
    reveal(ack_node); reveal(retry_node); reveal(failed_delivery); reveal(emitted_node);
    reveal(retirement::incident); reveal(node_rules);
    match n.phase {
        Phase::Closed(_) => {}, Phase::Stopping(_) => {},
        Phase::Ingesting => {}, Phase::Auditing => {}, Phase::Unplanned => {},
        Phase::Planning { .. } => {}, Phase::Ready(_) => {}, Phase::Making => {},
        Phase::Checking => {}, Phase::Reviewing => {}, Phase::Publishing => {},
        Phase::Queued => {}, Phase::Rebasing => {}, Phase::AwaitingRob => {},
    }
}

/// State-table permanent failure: owner question is typed, fresh and costs no credit.
pub(super) proof fn permanent_rules(w: World, n: Node, q: u64, seq: u64)
    requires node_rules(w, n), node_headroom(n), q < w.next_question,
    ensures node_rules(w, ack_node(n, q, seq, Ack::Permanent)),
{
    reveal(ack_node); reveal(permanent_node); reveal(node_rules);
    match n.phase {
        Phase::Closed(_) => {}, Phase::Stopping(_) => {},
        Phase::Ingesting => {}, Phase::Auditing => {}, Phase::Unplanned => {},
        Phase::Planning { .. } => {}, Phase::Ready(_) => {}, Phase::Making => {},
        Phase::Checking => {}, Phase::Reviewing => {}, Phase::Publishing => {},
        Phase::Queued => {}, Phase::Rebasing => {}, Phase::AwaitingRob => {},
    }
}

/// #629 / Q54: OpenPr refusal preserves every checker rule independently of fail_fast.
pub(super) proof fn refusal_rules(w: World, n: Node, q: u64, seq: u64)
    requires node_rules(w, n), node_headroom(n), asks(n, seq, Ack::BodyPolicyRefused) ==> q < w.next_question,
    ensures node_rules(w, ack_node(n, q, seq, Ack::BodyPolicyRefused)),
{
    if !publication(n, seq, Ack::BodyPolicyRefused) {
        if asks(n, seq, Ack::BodyPolicyRefused) { permanent_rules(w, n, q, seq); }
        else { clear_rules(w, n); }
        reveal(ack_node);
    } else {
        reveal(ack_node); reveal(node_rules); reveal(retirement::stopped);
    }
}

/// State-table acknowledgements / #806: exact immutable fields and all common node/Q54 rules.
pub(super) proof fn ack_transition(w: World, n: Node, q: u64, seq: u64, ack: Ack)
    requires node_rules(w, n), node_headroom(n),
    ensures node_transition(n, ack_node(n, q, seq, ack)),
        q54(n, ack_node(n, q, seq, ack), w.fail_fast, publication(n, seq, ack)),
        ack_node(n, q, seq, ack).credit == n.credit,
        ack_node(n, q, seq, ack).repo == n.repo,
        ack_node(n, q, seq, ack).footprint == n.footprint,
        !ack_node(n, q, seq, ack).holds || n.holds,
        !ack_node(n, q, seq, ack).pool || n.pool,
        ack_node(n, q, seq, ack).ticket.is_none() || ack_node(n, q, seq, ack).ticket == n.ticket,
{
    reveal(ack_node); reveal(confirmed_node); reveal(confirmed_cleanup); reveal(confirmed_agenda_node);
    reveal(retry_node); reveal(failed_delivery); reveal(emitted_node); reveal(permanent_node);
    reveal(retirement::stopped); reveal(retirement::incident);
    reveal(node_rules); reveal(node_transition); reveal(q54);
    match ack { Ack::Confirmed => {}, Ack::Retryable => {}, Ack::Permanent => {}, Ack::BodyPolicyRefused => {} }
}

/// State-table global Ack safety / #806: no new holder, pool grant or duplicate ticket.
pub(super) proof fn ack_world(b: World, a: World, seq: u64, ack: Ack, ob: Outbox, oa: Outbox)
    requires domain(b), accepted(b), bounded(b), ack_post(b, a, 0, seq, ack, ob, oa),
    ensures domain(a), accepted(a),
        forall|j: int| 0 <= j < 2 ==> node_transition(b.nodes@[j], a.nodes@[j])
            && q54(b.nodes@[j], a.nodes@[j], b.fail_fast, j == 0 && publication(b.nodes@[0], seq, ack)),
{
    reveal(ack_post); reveal(world_frame); reveal(accepted);
    question_context(b, a, b.nodes@[0]);
    question_context(b, a, b.nodes@[1]);
    match ack {
        Ack::Confirmed => confirmed_rules(a, b.nodes@[0], b.next_question, seq),
        Ack::Retryable => retry_rules(a, b.nodes@[0], b.next_question, seq),
        Ack::Permanent => {
            if asks(b.nodes@[0], seq, ack) { permanent_rules(a, b.nodes@[0], b.next_question, seq); }
            else { clear_rules(a, b.nodes@[0]); reveal(ack_node); }
        },
        Ack::BodyPolicyRefused => refusal_rules(a, b.nodes@[0], b.next_question, seq),
    }
    ack_transition(a, b.nodes@[0], b.next_question, seq, ack);
    reveal(node_transition); reveal(q54);
    assert forall|j: int| 0 <= j < 2 implies node_transition(b.nodes@[j], a.nodes@[j])
        && q54(b.nodes@[j], a.nodes@[j], b.fail_fast, j == 0 && publication(b.nodes@[0], seq, ack)) by {
        assert(j == 0 || j == 1);
    }
    assert forall|r: int| 0 <= r < 2 implies holders(a, RepoId(r as u64), 2) <= a.gates@[r].w by {
        assert(holders(a, RepoId(r as u64), 2) <= holders(b, RepoId(r as u64), 2));
    }
    footprint::storage_frame(b, a, b.nodes@[0].footprint, b.nodes@[1].footprint);
}
}
