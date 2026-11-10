//! Executable ingest transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Ingest reply rows / #771: initialize the same one-time deposit before audit.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn finish_ingest(w: &mut World, i: usize, size: u8, early_end: Option<EarlyEnd>)
    -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
    ensures (ordinary_await(old(w).nodes@[i as int]) && old(w).nodes@[i as int].phase == Phase::Ingesting
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, 0, 0),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds);] {
    kernel_snapshot! { before_ingest = *w }
    finish_ingest_node(&mut w.nodes[i], size, early_end);
    w.withhold_dependency_credit(i);
    kernel_proof! {
        assert forall|j: int| 0 <= j < before_ingest@.nodes@.len() && j != i implies
            w.nodes@[j] == before_ingest@.nodes@[j] by {}
    }
}
}

kernel_fn! {
/// Ingest reply rows / #771: the unchanged one-node initialization before
/// dependency credit reconciliation. All non-credit counters retain their frame.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn finish_ingest_node(m: &mut Node, size: u8, early_end: Option<EarlyEnd>) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures *final(m) == (Node {
        credit: Credit { ready: true,
            start: if old(m).credit.ready { old(m).credit.start } else { (size as u64 + RESERVE) as u64 },
            ..old(m).credit },
        early_end, phase: Phase::Auditing, pos: 0, ticket: None,
        generation: (old(m).generation + 1) as u64,
        effect: match old(m).effect {
            Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        },
        ..*old(m)
    }),
] {
    kernel_proof! { timing3_lemmas::ingest_deposit_bound(size); }
    if !m.credit.ready {
        m.credit.ready = true;
        m.credit.start = size as u64 + RESERVE;
    }
    m.early_end = early_end;
    set_phase(m, Phase::Auditing);
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn reply_ingest(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        matches!(reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures reply_family_safety::entry_rows(*old(w), *final(w), i as int, reply),
        port_t860_contracts::transition_post(*old(w), *final(w), i as int, reply, *old(out), *final(out)),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (reply == Reply::AuditDisagrees && old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1 ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
] {
    kernel_snapshot! { before_ingest_entry = *w }
    kernel_snapshot! { before_ingest_out = *out }
    let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
        reply: reply, fast: w.fail_fast, next_question: w.next_question };
    let (mut node, note) = reply_ingest_parts(w.nodes[i], c);
    match reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) => {
            let delivered = w.dependencies_delivered(NodeId(i as u64));
            node = withhold_ingest_v(node, delivered);
        },
        _ => {},
    }
    w.nodes[i] = node;
    match note { Some(value) => notice(out, i, value), None => {} }
    kernel_proof! {
        reply_family_ingest_world::ingest_done(before_ingest_entry@.nodes@[i as int], reply);
        reveal(reply_family_ingest_world::final_node);
        reveal(reply_family_dispatch::other_rows);
        reply_family_ingest_world::ingest_world(before_ingest_entry@, *w, i, reply,
            before_ingest_out@, *out);
    }
}
}

kernel_fn! {
/// Audit disagreement: isolate the verbatim node update from mutable world frames.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn audit_disagreement_node(m: &mut Node) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures ingest_facts::audit_post(*old(m), *final(m)),
] {
    kernel_snapshot! { before_audit = *m }
    increment_audit(m);
    kernel_snapshot! { before_phase = *m }
    if m.audit_rounds < 2 {
        // Ingest revises once with the audit's objections.
        set_phase(m, Phase::Ingesting);
        kernel_proof! { ingest_facts::audit_first(before_audit@, before_phase@, *m); }
    } else {
        // Intake cannot split. The one repair is spent.
        stop_for_review(m, ReviewStop::AuditDisagreement);
        kernel_proof! { ingest_facts::audit_spent(before_audit@, before_phase@, *m); }
    }
}
}

kernel_fn! {
/// Audit disagreement: verbatim saturation isolated from world frames.
fn increment_audit(m: &mut Node) -> (result: ()) [
    ensures ingest_facts::incremented(*old(m), *final(m)),
        final(m).audit_rounds == if old(m).audit_rounds < u8::MAX {
            (old(m).audit_rounds + 1) as u8
        } else { u8::MAX },
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        settle_shape(*old(m)) == settle_shape(*final(m)),
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).last_cmd == old(m).last_cmd,
        final(m).credit == old(m).credit,
] proof [reveal(ingest_facts::incremented);] {
    m.audit_rounds = m.audit_rounds.saturating_add(1);
}
}

#[cfg(test)]
#[path = "ingest_equivalence/mod.rs"]
mod reply_ingest_equivalence;

kernel_fn! {
/// Reply node work, with completed-stage rows separate from world mutation.
pub(super) fn reply_ingest_node(n: Node, c: ReplyCtx) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures result == reply_family_values::ingest(n, c.reply),
        reply_family_values::callable(result),
] proof [reveal(reply_family_values::ingest);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let i = c.node;
    let reply = c.reply;
    match reply {
        Reply::IngestDone { size } => {
            node = finish_ingest_v(node, size, None);
            return ReplyChange::Done { node, note };
        },
        Reply::IngestEarlyEnd(e) => {
            node = finish_ingest_v(node, 1, Some(e));
            return ReplyChange::Done { node, note };
        },
        Reply::AuditAgrees => match node.early_end {
            Some(e) => {
                node = enter_stopping_v(node, e.outcome());
                return ReplyChange::Done { node, note };
            },
            None => {
                note = Some(Notice::MeasuresPosted);
                node = set_phase_v(node, Phase::Unplanned);
                return ReplyChange::Done { node, note };
            }
        },
        Reply::AuditDisagrees => {
            node = audit_disagreement_v(node);
            return ReplyChange::Done { node, note };
        },
        Reply::ProtectedPaths => {
            node = enter_stopping_v(node, Outcome::NotActionable);
            return ReplyChange::Done { node, note };
        },
        _ => {}
    }
    ReplyChange::Done { node, note }
}
}


kernel_fn! {
fn finish_ingest_v(n: Node, size: u8, early_end: Option<EarlyEnd>) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == reply_family_values::ingested(n, size, early_end),
] {
    let mut m = n;
    finish_ingest_node(&mut m, size, early_end);
    m
}
}

kernel_fn! {
fn audit_disagreement_v(n: Node) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == port_t860_contracts::audit_node(n),
] {
    let mut m = n;
    audit_disagreement_node(&mut m);
    kernel_proof! { ingest_facts::audit_result(n, m); }
    m
}
}

kernel_fn! {
/// The unchanged ingest arms, before the table and notice writes.
fn reply_ingest_parts(n: Node, c: ReplyCtx) -> (result: (Node, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures (ReplyChange::Done { node: result.0, note: result.1 }) == reply_family_values::ingest(n, c.reply),
] proof [reveal(reply_family_values::ingest);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let i = c.node;
    let reply = c.reply;
    match reply {
        Reply::IngestDone { size } => {
            node = finish_ingest_v(node, size, None);
            return (node, note);
        },
        Reply::IngestEarlyEnd(e) => {
            node = finish_ingest_v(node, 1, Some(e));
            return (node, note);
        },
        Reply::AuditAgrees => match node.early_end {
            Some(e) => {
                node = enter_stopping_v(node, e.outcome());
                return (node, note);
            },
            None => {
                note = Some(Notice::MeasuresPosted);
                node = set_phase_v(node, Phase::Unplanned);
                return (node, note);
            }
        },
        Reply::AuditDisagrees => {
            node = audit_disagreement_v(node);
            return (node, note);
        },
        Reply::ProtectedPaths => {
            node = enter_stopping_v(node, Outcome::NotActionable);
            return (node, note);
        },
        _ => {}
    }
    (node, note)
}
}

kernel_fn! {
/// Dependency withholding on a local node, before its single table write.
fn withhold_ingest_v(n: Node, delivered: bool) -> (result: Node) [
    ensures result == reply_family_ingest_world::withheld(n, delivered),
] proof [reveal(reply_family_ingest_world::withheld);] {
    let mut node = n;
    if !delivered && !node.planning_started
        && matches!(node.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned)
    {
        if node.credit.ready { node.credit.withheld = false; }
    }
    node
}
}
