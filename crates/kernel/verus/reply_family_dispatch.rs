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
    ensures !reply_family_safety::rows(n, c, reply_family_values::work(n, c.fast, c.reply)),
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
}
