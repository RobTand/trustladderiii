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
    ensures (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::IngestDone { size } => {
            finish_ingest(w, i, size, None);
        }
        Reply::IngestEarlyEnd(e) => {
            finish_ingest(w, i, 1, Some(e));
        }
        Reply::AuditAgrees => match w.nodes[i].early_end {
            Some(e) => enter_stopping(&mut w.nodes[i], e.outcome()),
            None => {
                notice(out, i, Notice::MeasuresPosted);
                set_phase(&mut w.nodes[i], Phase::Unplanned);
            }
        },
        Reply::AuditDisagrees => {
            let m = &mut w.nodes[i];
            m.audit_rounds = m.audit_rounds.saturating_add(1);
            if m.audit_rounds < 2 {
                // Ingest revises once with the audit's objections.
                set_phase(m, Phase::Ingesting);
            } else {
                let cont = match m.early_end {
                    Some(e) => Cont::Stop(e.outcome()),
                    None => Cont::Unplanned,
                };
                ask_rob(w, i, QKind::AuditDisagreement, cont, 0, out);
            }
        }
        Reply::ProtectedPaths => enter_stopping(&mut w.nodes[i], Outcome::NotActionable),
        _ => {}
    }
}
}
