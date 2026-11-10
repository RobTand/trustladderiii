//! Dispatcher facts stated independently of executable updates.
use super::*;

verus! {
/// Literal node-pair rows; callers supply fields rather than a whole-Node equation.
pub(super) proof fn pair_rows(b: Node, z: Node)
    requires reply_family_values::frames(b, z), !(b.phase is Closed),
    ensures transition_safety::node_step(b, z), z.next_seq == b.next_seq,
        z.repo == b.repo, z.last_cmd == b.last_cmd,
{
}

pub(super) proof fn work_bounds(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        !matches!(c.reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::work(n, c.fast, c.reply)),
        reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply)).footprint == n.footprint,
{
    reveal(reply_family_values::work);
    match c.reply {
        Reply::MakerCantMeet => reply_family_posts::rung_value_posts(n, c.fast, c.next_question, ReviewStop::MakerCantMeet),
        Reply::MakerOutOfRoom => reply_family_posts::room_bound(n, c.fast, c.next_question, ReviewStop::MakerOutOfRoom),
        Reply::ChecksDone { pass, in_footprint, .. } => if !(pass && in_footprint) {
            reply_family_posts::failed_round_value_posts(n, c.fast, c.next_question, ReviewStop::ChecksFailed);
        },
        Reply::ChangesRequested => reply_family_posts::failed_round_value_posts(n, c.fast, c.next_question, ReviewStop::ChangesRequested),
        Reply::ReviewerOutOfRoom => if n.review_restarted {
            reply_family_posts::room_bound(n, c.fast, c.next_question, ReviewStop::ReviewerOutOfRoom);
        } else {
            reply_family_posts::recheck_value_posts(Node { review_restarted: true, ..n }, c.next_question, Phase::Reviewing);
        },
        Reply::MakerDone { .. } | Reply::Approved { .. } => {},
        _ => reply_family_posts::rebase_value_posts(n, c.next_question, c.reply),
    }
}

pub(super) proof fn work_progress(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        !matches!(c.reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures successful_reply_input(n, c.reply) ==>
        distance(reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply))) < distance(n),
{
    reveal(reply_family_values::work);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
    if matches!(c.reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed) {
        reply_family_posts::rebase_value_posts(n, c.next_question, c.reply);
    }
}

pub(super) proof fn work_maker_rows(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::MakerDone { .. } | Reply::MakerCantMeet | Reply::MakerOutOfRoom),
    ensures reply_family_safety::rows(n, c, reply_family_values::work(n, c.fast, c.reply)),
{
    work_bounds(n, c);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply));
        pair_rows(b, z);
        reveal(reply_family_values::work); reveal(reply_family_values::rebase);
        reveal(reply_family_values::room); reveal(reply_family_values::rung);
        reveal(reply_family_values::failed_round); reveal(reply_family_values::recheck);
        reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_checks_rows(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::ChecksDone { .. }),
    ensures reply_family_safety::rows(n, c, reply_family_values::work(n, c.fast, c.reply)),
{
    work_bounds(n, c);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply));
        pair_rows(b, z);
        reveal(reply_family_values::work); reveal(reply_family_values::rebase);
        reveal(reply_family_values::room); reveal(reply_family_values::rung);
        reveal(reply_family_values::failed_round); reveal(reply_family_values::recheck);
        reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_review_rows(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::Approved { .. } | Reply::ChangesRequested | Reply::ReviewerOutOfRoom),
    ensures reply_family_safety::rows(n, c, reply_family_values::work(n, c.fast, c.reply)),
{
    work_bounds(n, c);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply));
        pair_rows(b, z);
        reveal(reply_family_values::work); reveal(reply_family_values::rebase);
        reveal(reply_family_values::room); reveal(reply_family_values::rung);
        reveal(reply_family_values::failed_round); reveal(reply_family_values::recheck);
        reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn rebase_rows(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
    ensures reply_family_safety::rows(n, c, reply_family_values::rebase(n, c.reply)),
{
    reply_family_posts::rebase_value_posts(n, c.next_question, c.reply);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::rebase(n, c.reply));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::rebase(n, c.reply));
        pair_rows(b, z);
        reveal(reply_family_values::work); reveal(reply_family_values::rebase);
        reveal(reply_family_values::room); reveal(reply_family_values::rung);
        reveal(reply_family_values::failed_round); reveal(reply_family_values::recheck);
        reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_completion(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        !matches!(c.reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_safety::rows(n, c, reply_family_values::work(n, c.fast, c.reply)),
{
    match c.reply {
        Reply::MakerDone { .. } | Reply::MakerCantMeet | Reply::MakerOutOfRoom => work_maker_rows(n, c),
        Reply::ChecksDone { .. } => work_checks_rows(n, c),
        Reply::Approved { .. } | Reply::ChangesRequested | Reply::ReviewerOutOfRoom => work_review_rows(n, c),
        _ => { rebase_rows(n, c); reveal(reply_family_values::work); },
    }
}

/// All moved work facts, at the canonical result; no node equation premise.
pub(super) proof fn work_value_posts(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        !matches!(c.reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::work(n, c.fast, c.reply)),
        reply_family_safety::rows(n, c, reply_family_values::work(n, c.fast, c.reply)),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply))) < distance(n)),
        reply_family_values::finish(c.next_question, reply_family_values::work(n, c.fast, c.reply)).footprint == n.footprint,
{
    work_bounds(n, c); work_progress(n, c); work_completion(n, c);
}

/// All moved rebase facts, at the canonical result; no node equation premise.
pub(super) proof fn rebase_value_posts(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::rebase(n, c.reply)),
        reply_family_safety::rows(n, c, reply_family_values::rebase(n, c.reply)),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, reply_family_values::rebase(n, c.reply))) < distance(n)),
        reply_family_values::finish(c.next_question, reply_family_values::rebase(n, c.reply)).footprint == n.footprint,
{
    reply_family_posts::rebase_value_posts(n, c.next_question, c.reply);
    rebase_rows(n, c);
}
pub(super) proof fn plan_bounds(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::plan(n, c.fast, c.reply)),
        c.reply == Reply::PlanCantMeet && c.fast && n.phase is Planning ==>
            reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).phase == Phase::Stopping(Outcome::Stopped)
                && reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).review_stop == Some(ReviewStop::PlanCantMeet),

        reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).footprint == n.footprint
            || match c.reply { Reply::PlanLeaf { footprint, .. } => reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).footprint == footprint, _ => false },

{
    reveal(reply_family_values::plan);
    match c.reply {
        Reply::PlanCantMeet => reply_family_posts::plan_retry_value_posts(n, c.fast, c.next_question, true),
        Reply::PlanLeaf { footprint, fits } => {
            if !(fits && !footprint::footprint_empty(footprint) && !matches!(n.phase, Phase::Planning { must_split: true })) {
                reply_family_posts::plan_retry_value_posts(n, c.fast, c.next_question, false);
            }
        },
        _ => {},
    }
}

pub(super) proof fn plan_progress(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures successful_reply_input(n, c.reply) ==>
        distance(reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply))) < distance(n),
{
    reveal(reply_family_values::plan);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
    footprint::inline_is_valid(vstd::pervasive::arbitrary(), 1);
}

pub(super) proof fn plan_completion(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures reply_family_safety::rows(n, c, reply_family_values::plan(n, c.fast, c.reply)),
{
    plan_bounds(n, c);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply));
        pair_rows(b, z);
        reveal(reply_family_values::plan);
        reveal(reply_family_values::plan_retry);
        reveal(retirement::stopped); reveal(port_t860_contracts::audit_node);
        reveal(distance);
    }
}

pub(super) proof fn plan_value_posts(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::plan(n, c.fast, c.reply)),
        reply_family_safety::rows(n, c, reply_family_values::plan(n, c.fast, c.reply)),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply))) < distance(n)),
        (c.reply == Reply::PlanCantMeet && c.fast && n.phase is Planning ==>
            reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).phase == Phase::Stopping(Outcome::Stopped)
                && reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).review_stop == Some(ReviewStop::PlanCantMeet)),
        reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).footprint == n.footprint
            || match c.reply { Reply::PlanLeaf { footprint, .. } => reply_family_values::finish(c.next_question, reply_family_values::plan(n, c.fast, c.reply)).footprint == footprint, _ => false },

{
    plan_bounds(n, c); plan_progress(n, c); plan_completion(n, c);
}

/// The deposit case pays only its arithmetic and phase facts.
pub(super) proof fn ingest_deposit_bounds(n: Node, nq: u64, size: u8, early_end: Option<EarlyEnd>)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::reply_bound(n, nq,
        ReplyChange::Done { node: reply_family_values::ingested(n, size, early_end), note: None }),
{
    timing3_lemmas::ingest_deposit_bound(size);
    if n.credit.ready {} else {}
}

pub(super) proof fn ingest_agrees_bounds(n: Node, nq: u64)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::reply_bound(n, nq, reply_family_values::ingest(n, Reply::AuditAgrees)),
        reply_family_values::finish(nq, reply_family_values::ingest(n, Reply::AuditAgrees)).footprint == n.footprint,
{
    reveal(reply_family_values::ingest);
    match n.early_end {
        Some(_) => { reveal(retirement::stopped); },
        None => {},
    }
}

pub(super) proof fn ingest_disagrees_bounds(n: Node, nq: u64)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::reply_bound(n, nq, reply_family_values::ingest(n, Reply::AuditDisagrees)),
        reply_family_values::finish(nq, reply_family_values::ingest(n, Reply::AuditDisagrees)).footprint == n.footprint,
{
    reveal(reply_family_values::ingest); reveal(port_t860_contracts::audit_node);
    if n.audit_rounds == 0 {} else { reveal(retirement::stopped); }
}

pub(super) proof fn ingest_protected_bounds(n: Node, nq: u64)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::reply_bound(n, nq, reply_family_values::ingest(n, Reply::ProtectedPaths)),
        reply_family_values::finish(nq, reply_family_values::ingest(n, Reply::ProtectedPaths)).footprint == n.footprint,
{
    reveal(reply_family_values::ingest); reveal(retirement::stopped);
}

pub(super) proof fn ingest_bounds(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::ingest(n, c.reply)),
        reply_family_values::finish(c.next_question, reply_family_values::ingest(n, c.reply)).footprint == n.footprint,
{
    reveal(reply_family_values::ingest);
    match c.reply {
        Reply::IngestDone { size } => ingest_deposit_bounds(n, c.next_question, size, None),
        Reply::IngestEarlyEnd(e) => ingest_deposit_bounds(n, c.next_question, 1, Some(e)),
        Reply::AuditAgrees => ingest_agrees_bounds(n, c.next_question),
        Reply::AuditDisagrees => ingest_disagrees_bounds(n, c.next_question),
        Reply::ProtectedPaths => ingest_protected_bounds(n, c.next_question),
        _ => {},
    }
}

pub(super) proof fn ingest_progress(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures successful_reply_input(n, c.reply) ==>
        distance(reply_family_values::finish(c.next_question, reply_family_values::ingest(n, c.reply))) < distance(n),
{
    reveal(reply_family_values::ingest);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
    reveal(retirement::stopped); reveal(port_t860_contracts::audit_node);
}

pub(super) proof fn ingest_completion(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures reply_family_safety::rows(n, c, reply_family_values::ingest(n, c.reply)),
{
    ingest_bounds(n, c);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::ingest(n, c.reply));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::ingest(n, c.reply));
        pair_rows(b, z);
        reveal(reply_family_values::ingest);
        reveal(reply_family_values::plan_retry);
        reveal(retirement::stopped); reveal(port_t860_contracts::audit_node);
        reveal(distance);
    }
}

/// Withholding dependency credit changes no input of the progress measure.
pub(super) proof fn withheld_distance(n: Node, withheld: bool)
    ensures distance(Node { credit: Credit { withheld, ..n.credit }, ..n }) == distance(n),
{
    reveal(distance);
}

pub(super) proof fn ingest_value_posts(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::ingest(n, c.reply)),
        reply_family_values::finish(c.next_question, reply_family_values::ingest(n, c.reply)).footprint == n.footprint,
        reply_family_safety::rows(n, c, reply_family_values::ingest(n, c.reply)),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, reply_family_values::ingest(n, c.reply))) < distance(n)),
{
    ingest_bounds(n, c); ingest_progress(n, c); ingest_completion(n, c);
}

pub(super) proof fn plan_check_bounds(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::plan_check(n)),
        reply_family_values::finish(c.next_question, reply_family_values::plan_check(n)).footprint != n.footprint,
{
    reveal(reply_family_values::plan_check);
    reply_family_facts::stopped_review(n, ReviewStop::PlanCantMeet);
}

pub(super) proof fn plan_check_progress(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures successful_reply_input(n, c.reply) ==>
        distance(reply_family_values::finish(c.next_question, reply_family_values::plan_check(n))) < distance(n),
{
    reveal(reply_family_values::plan_check);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
    reveal(retirement::stopped); reveal(port_t860_contracts::audit_node);
}

pub(super) proof fn plan_check_completion(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_safety::rows(n, c, reply_family_values::plan_check(n)),
{
    plan_check_bounds(n, c);
    reveal(reply_family_safety::completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && reply_family_safety::stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, reply_family_values::plan_check(n));
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended);
        let b = before.nodes@[c.node as int];
        let z = reply_family_values::finish(c.next_question, reply_family_values::plan_check(n));
        pair_rows(b, z);
        reveal(reply_family_values::plan_check);
        reveal(reply_family_values::plan_retry);
        reveal(retirement::stopped); reveal(port_t860_contracts::audit_node);
        reveal(distance);
    }
}

pub(super) proof fn plan_check_value_posts(n: Node, c: ReplyCtx)
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_values::reply_bound(n, c.next_question, reply_family_values::plan_check(n)),
        reply_family_safety::rows(n, c, reply_family_values::plan_check(n)),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, reply_family_values::plan_check(n))) < distance(n)),
        reply_family_values::finish(c.next_question, reply_family_values::plan_check(n)).footprint == n.footprint,
{
    plan_check_bounds(n, c); plan_check_progress(n, c); plan_check_completion(n, c);
}

/// Off-index storage and world counters, separate from the handled node's fields.
#[verifier::opaque]
pub(super) open spec fn other_rows(b: World, z: World, i: int) -> bool {
    z.nodes@.len() == b.nodes@.len()
        && (forall|j: int| 0 <= j < b.nodes@.len() && j != i
            ==> #[trigger] z.nodes@[j] == b.nodes@[j])
        && z.next_ticket == b.next_ticket && z.gates@ == b.gates@
        && z.pool_capacity == b.pool_capacity && z.fail_fast == b.fail_fast
        && z.file_sets@ == b.file_sets@ && z.dependencies@ == b.dependencies@
}

/// World bounds from literal handled-node goals; no target Node equation premise.
pub(super) proof fn reply_lift(b: World, z: World, i: int, ob: usize, oa: usize)
    requires 0 <= i < b.nodes@.len(), other_rows(b, z, i),
        (settle_shape(b.nodes@[i]) ==> settle_shape(z.nodes@[i])),
        z.nodes@[i].repo == b.nodes@[i].repo,
        z.nodes@[i].generation as int <= b.nodes@[i].generation as int + 1,
        z.nodes@[i].next_seq == b.nodes@[i].next_seq,
        z.nodes@[i].last_cmd == b.nodes@[i].last_cmd,
        z.nodes@[i].credit.start as int <= b.nodes@[i].credit.start as int + CREDIT_START_STEP_BUDGET,
        z.nodes@[i].credit.earned == b.nodes@[i].credit.earned,
        z.nodes@[i].credit.added == b.nodes@[i].credit.added,
        z.nodes@[i].credit.spent as int <= b.nodes@[i].credit.spent as int + 1,
        z.next_question as int <= b.next_question as int + 1,
        oa as int <= ob as int + 1,
    ensures reply_bounds(b, z, i, ob, oa),
{
    reveal(other_rows); reveal(reply_bounds);
}

}
