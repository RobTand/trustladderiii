//! Ingest entry: literal node-pair rows and one world contract.
use super::*;

verus! {
#[verifier::opaque]
pub(super) open spec fn withheld(n: Node, delivered: bool) -> Node {
    Node { credit: Credit { withheld: n.credit.withheld || (!delivered
        && !n.planning_started && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned)
        && n.credit.ready), ..n.credit }, ..n }
}

#[verifier::opaque]
pub(super) open spec fn final_node(n: Node, reply: Reply, delivered: bool) -> Node {
    let z = reply_family_values::finish(0, reply_family_values::ingest(n, reply));
    match reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) => withheld(z, delivered),
        _ => z,
    }
}

pub(super) proof fn ingest_done(n: Node, reply: Reply)
    ensures reply_family_values::ingest(n, reply) is Done,
{
    reveal(reply_family_values::ingest);
}

pub(super) proof fn ingest_pair_bounds(b: Node, z: Node, c: ReplyCtx, delivered: bool)
    requires node_headroom(b), arithmetic_consistent(b), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
        z.repo == final_node(b, c.reply, delivered).repo,
        z.footprint == final_node(b, c.reply, delivered).footprint,
        z.generation == final_node(b, c.reply, delivered).generation,
        z.next_seq == final_node(b, c.reply, delivered).next_seq,
        z.last_cmd == final_node(b, c.reply, delivered).last_cmd,
        z.credit.start == final_node(b, c.reply, delivered).credit.start,
        z.credit.earned == final_node(b, c.reply, delivered).credit.earned,
        z.credit.added == final_node(b, c.reply, delivered).credit.added,
        z.credit.spent == final_node(b, c.reply, delivered).credit.spent,
        z.phase == final_node(b, c.reply, delivered).phase,
        z.pos == final_node(b, c.reply, delivered).pos,
        z.pool == final_node(b, c.reply, delivered).pool,
        z.holds == final_node(b, c.reply, delivered).holds,
    ensures z.repo == b.repo, z.footprint == b.footprint, z.last_cmd == b.last_cmd,
        (settle_shape(b) ==> settle_shape(z)),
        z.generation as int <= b.generation as int + 1, z.next_seq == b.next_seq,
        z.credit.start as int <= b.credit.start as int + CREDIT_START_STEP_BUDGET,
        z.credit.earned == b.credit.earned, z.credit.added == b.credit.added,
        z.credit.spent as int <= b.credit.spent as int + 1,
{
    reply_family_dispatch::ingest_bounds(b, c);
    ingest_done(b, c.reply);
    reveal(final_node); reveal(withheld);
}

pub(super) proof fn ingest_pair_progress(b: Node, z: Node, c: ReplyCtx, delivered: bool)
    requires node_headroom(b), arithmetic_consistent(b), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
        z.phase == final_node(b, c.reply, delivered).phase,
        z.pos == final_node(b, c.reply, delivered).pos,
        z.paused == final_node(b, c.reply, delivered).paused,
        z.waiting_incident == final_node(b, c.reply, delivered).waiting_incident,
        z.effect == final_node(b, c.reply, delivered).effect,
        z.question == final_node(b, c.reply, delivered).question,
    ensures !successful_reply_input(b, c.reply) ==> distance(z) < distance(b),
{
    reply_family_dispatch::ingest_progress(b, c);
    ingest_done(b, c.reply);
    reveal(final_node); reveal(withheld); reveal(distance);
}

pub(super) proof fn ingest_pair_completed(b: Node, z: Node, c: ReplyCtx, delivered: bool)
    requires node_headroom(b), arithmetic_consistent(b), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
        z.phase == final_node(b, c.reply, delivered).phase,
        z.proposed == final_node(b, c.reply, delivered).proposed,
        z.adopted == final_node(b, c.reply, delivered).adopted,
        z.next_seq == final_node(b, c.reply, delivered).next_seq,
        z.credit == final_node(b, c.reply, delivered).credit,
        z.review_stop == final_node(b, c.reply, delivered).review_stop,
        z.attempt == final_node(b, c.reply, delivered).attempt,
        z.replanned == final_node(b, c.reply, delivered).replanned,
        z.question == final_node(b, c.reply, delivered).question,
        z.plan_retries == final_node(b, c.reply, delivered).plan_retries,
    ensures reply_family_safety::completed(b, c, z),
{
    reply_family_dispatch::ingest_completion(b, c);
    ingest_done(b, c.reply);
    reveal(final_node); reveal(withheld);
    reveal(reply_family_safety::completed);
    match c.reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) => {
            reveal(reply_family_values::ingest);
            assert forall|before: World|
                #[trigger] retirement::node_rules(before, before.nodes@[c.node as int])
                    && reply_family_safety::stage(before, b, c) implies {
                        let origin = before.nodes@[c.node as int];
                        transition_safety::node_step(origin, z) && z.next_seq == origin.next_seq
                            && transition_safety::fail_fast_node(origin, z, c.fast,
                                Event::Reply { node: NodeId(c.node as u64), generation: c.generation,
                                    reply: c.reply }, c.node as int)
                            && transition_safety::reply_rules(origin, z, c.fast, c.generation, c.reply)
                    } by {
                reveal(port_t860_contracts::ended); reveal(retirement::node_rules);
            }
        },
        _ => {},
    }
}

pub(super) proof fn ingest_audit_rows(b: Node, reply: Reply, delivered: bool)
    ensures reply == Reply::AuditDisagrees ==> final_node(b, reply, delivered) == port_t860_contracts::audit_node(b),
        reply == Reply::AuditDisagrees && b.phase == Phase::Auditing && b.audit_rounds >= 1 ==> {
            let z = final_node(b, reply, delivered);
            z.phase == Phase::Stopping(Outcome::Stopped) && z.question.is_none()
                && z.review_stop == Some(ReviewStop::AuditDisagreement) && z.credit == b.credit
        },
        reply == Reply::AuditDisagrees ==> reply_family_values::ingest(b, reply)
            == (ReplyChange::Done { node: port_t860_contracts::audit_node(b), note: None }),

{
    reveal(final_node);
    if reply == Reply::AuditDisagrees {
        reveal(reply_family_values::ingest);
        reveal(port_t860_contracts::audit_node); reveal(retirement::stopped);
    }
}

/// reply_ingest's ensures verbatim, with old/final renamed only.
pub(super) proof fn ingest_world(before: World, after: World, i: usize, reply: Reply,
    ob: Outbox, oa: Outbox)
    requires i < before.nodes@.len(), ob.len < OUTBOX,
        node_headroom(before.nodes@[i as int]), arithmetic_consistent(before.nodes@[i as int]),
        before.next_question < u64::MAX,
        matches!(reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
        after.nodes@ == before.nodes@.update(i as int,
            final_node(before.nodes@[i as int], reply, dependencies::prerequisites_delivered(before, NodeId(i as u64)))),
        reply_family_dispatch::other_rows(before, after, i as int),
        after.next_question == before.next_question,
        reply_family_values::resolved(reply_family_values::ingest(before.nodes@[i as int], reply), i as int, ob, oa),
    ensures reply_family_safety::entry_rows(before, after, i as int, reply),
        port_t860_contracts::transition_post(before, after, i as int, reply, ob, oa),
        (successful_reply_input(before.nodes@[i as int], reply)
            ==> distance(after.nodes@[i as int]) < distance(before.nodes@[i as int])),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        reply_bounds(before, after, i as int, ob.len, oa.len),
        after.nodes@[i as int].last_cmd == before.nodes@[i as int].last_cmd,
        (reply == Reply::AuditDisagrees && before.nodes@[i as int].phase == Phase::Auditing
            && before.nodes@[i as int].audit_rounds >= 1 ==> {
                let n = after.nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == before.nodes@[i as int].credit
                    && after.next_question == before.next_question
                    && oa == ob
            }),
{
    let b = before.nodes@[i as int];
    let z = after.nodes@[i as int];
    let c = ReplyCtx { node: i, generation: b.generation, reply, fast: before.fail_fast,
        next_question: before.next_question };
    let delivered = dependencies::prerequisites_delivered(before, NodeId(i as u64));
    ingest_pair_bounds(b, z, c, delivered);
    ingest_pair_progress(b, z, c, delivered);
    ingest_pair_completed(b, z, c, delivered);
    ingest_audit_rows(b, reply, delivered);
    ingest_done(b, reply);
    reveal(port_t860_contracts::transition_post);
    reveal(reply_family_dispatch::other_rows);
    reply_family_dispatch::reply_lift(before, after, i as int, ob.len, oa.len);
}
}
