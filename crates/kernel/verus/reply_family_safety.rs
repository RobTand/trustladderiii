//! Reply-stage producers. The frozen node_safety arm waits for debt's exports.
use super::*;

verus! {
/// The actual admitted path, relating an original world to end_operation's
/// returned node. No checker result about the prepared node is required.
pub(super) open spec fn stage(before: World, n: Node, c: ReplyCtx) -> bool {
    0 <= c.node < before.nodes@.len()
        && c.fast == before.fail_fast && c.next_question == before.next_question
        && {
            let b = before.nodes@[c.node as int];
            !(b.phase is Closed || b.phase is Stopping)
                && current_item(b) == Some(Item::Await)
                && (b.run == Some(c.generation) || b.job == Some(c.generation))
                && port_t860_contracts::admissible(before, b, c.reply)
                && n == port_t860_contracts::ended(b)
        }
}

/// The origin is quantified because kernel_fn cannot erase a Ghost<World>
/// argument. The node_rules trigger names the exact original-world premise.
#[verifier::opaque]
pub(super) open spec fn completed(n: Node, c: ReplyCtx, z: Node) -> bool {
    forall|before: World| #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
        && stage(before, n, c) ==> {
            let b = before.nodes@[c.node as int];
            transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                && transition_safety::fail_fast_node(b, z, c.fast,
                    Event::Reply { node: NodeId(c.node as u64), generation: c.generation,
                        reply: c.reply }, c.node as int)
                && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
        }
}

pub(super) open spec fn rows(n: Node, c: ReplyCtx, r: ReplyChange) -> bool {
    completed(n, c, reply_family_values::finish(c.next_question, r))
}

pub(super) open spec fn entry_rows(before: World, after: World, i: int, reply: Reply) -> bool {
    completed(before.nodes@[i], ReplyCtx { node: i as usize,
        generation: before.nodes@[i].generation, reply, fast: before.fail_fast,
        next_question: before.next_question }, after.nodes@[i])
}

/// The admission guard is derived from the handler's real checks and the
/// completion producer. It is not an added family-arm premise.
pub(super) proof fn admitted_stage(before: World, n: Node, c: ReplyCtx)
    requires 0 <= c.node < before.nodes@.len(),
        c.fast == before.fail_fast, c.next_question == before.next_question,
        !(before.nodes@[c.node as int].phase is Closed || before.nodes@[c.node as int].phase is Stopping),
        current_item(before.nodes@[c.node as int]) == Some(Item::Await),
        before.nodes@[c.node as int].run == Some(c.generation)
            || before.nodes@[c.node as int].job == Some(c.generation),
        port_t860_contracts::admissible(before, before.nodes@[c.node as int], c.reply),
        n == port_t860_contracts::ended(before.nodes@[c.node as int]),
    ensures stage(before, n, c),
{ }

/// Legacy ladder forwarding deliberately exports no exact review-stop value.
/// plan_retry_node supplies its separate PlanCantMeet post to the consumer.
pub(super) proof fn plan_retry_ladder(n: Node, fast: bool, nq: u64, cant_meet: bool,
    r: ReplyChange, ob: Outbox, oa: Outbox, i: int)
    requires n.phase is Planning, r == reply_family_values::plan_retry(n, fast, cant_meet),
        reply_family_values::resolved(r, i, ob, oa),
    ensures handler_safety::ladder(fast, cant_meet, n, reply_family_values::finish(nq, r), ob, oa),
{
    reveal(reply_family_values::plan_retry); reveal(retirement::stopped);
    reveal(handler_safety::ladder);
}

pub(super) proof fn plan_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires matches!(c.reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
        r == reply_family_values::plan(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
        c.fast && c.reply == Reply::PlanCantMeet && n.phase is Planning ==>
            reply_family_values::finish(c.next_question, r).phase == Phase::Stopping(Outcome::Stopped)
                && reply_family_values::finish(c.next_question, r).review_stop == Some(ReviewStop::PlanCantMeet),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
        reveal(reply_family_values::plan_retry);
    }
}

pub(super) proof fn work_maker_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires matches!(c.reply, Reply::MakerDone { .. } | Reply::MakerCantMeet | Reply::MakerOutOfRoom),
        r == reply_family_values::work(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_checks_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires c.reply is ChecksDone,
        r == reply_family_values::work(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_review_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires matches!(c.reply, Reply::Approved { .. } | Reply::ChangesRequested | Reply::ReviewerOutOfRoom),
        r == reply_family_values::work(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_rebase_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires matches!(c.reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
        r == reply_family_values::work(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
        reveal(distance);
    }
}

pub(super) proof fn work_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires !matches!(c.reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
        r == reply_family_values::work(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
    ensures rows(n, c, r),
{
    match c.reply {
        Reply::MakerDone { .. } | Reply::MakerCantMeet | Reply::MakerOutOfRoom => work_maker_rows(n, c, r),
        Reply::ChecksDone { .. } => work_checks_rows(n, c, r),
        Reply::Approved { .. } | Reply::ChangesRequested | Reply::ReviewerOutOfRoom => work_review_rows(n, c, r),
        _ => work_rebase_rows(n, c, r),
    }
}

pub(super) proof fn ingest_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
        r == reply_family_values::ingest(n, c.reply),
        reply_family_values::reply_bound(n, c.next_question, r),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
        reveal(port_t860_contracts::audit_node);
    }
}

pub(super) proof fn check_rows(n: Node, c: ReplyCtx, r: ReplyChange)
    requires matches!(c.reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
        r == reply_family_values::plan_check(n),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
    }
}

pub(super) proof fn files_rows(n: Node, c: ReplyCtx, files: Footprint, fits: bool,
    union: Footprint, r: ReplyChange)
    requires c.reply == (Reply::MakerNeedsFiles { files, fits }),
        r == reply_family_values::files(n, c.fast, files, fits, union),
    ensures rows(n, c, r),
{
    reveal(completed);
    assert forall|before: World|
        #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
            && stage(before, n, c) implies {
                let b = before.nodes@[c.node as int];
                let z = reply_family_values::finish(c.next_question, r);
                transition_safety::node_step(b, z) && z.next_seq == b.next_seq
                    && transition_safety::fail_fast_node(b, z, c.fast,
                        Event::Reply { node: NodeId(c.node as u64), generation: c.generation, reply: c.reply }, c.node as int)
                    && transition_safety::reply_rules(b, z, c.fast, c.generation, c.reply)
            } by {
        reveal(retirement::node_rules); reveal(port_t860_contracts::admissible);
        reveal(port_t860_contracts::ended); reveal(retirement::stopped);
    }
}

pub(super) proof fn withhold_rows(n: Node, c: ReplyCtx, r: ReplyChange, z: Node)
    requires matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)),
        r == reply_family_values::ingest(n, c.reply), rows(n, c, r),
        z == (Node { credit: Credit { withheld: z.credit.withheld,
            ..reply_family_values::finish(c.next_question, r).credit },
            ..reply_family_values::finish(c.next_question, r) }),
    ensures completed(n, c, z),
{
    reveal(completed); reveal(port_t860_contracts::ended);
}

/// Done output suffixes need no premise about the existing prefix.
pub(super) proof fn done_outputs(b: Node, n: Node, r: ReplyChange, i: int, ob: Outbox, oa: Outbox)
    requires r is Done, ob.len < OUTBOX,
        reply_family_values::resolved(r, i, ob, oa),
    ensures transition_safety::node_outputs(b, n, i, ob, oa, false),
{
    assert forall|k: int| ob.len <= k < oa.len implies match #[trigger] oa.items@[k] {
        Some(Out::Effect { node, seq, kind, head }) => node.0 == i
            && transition_safety::effect_node_ok(b, n, seq, kind, head, false),
        _ => true,
    } by { }
}

}
