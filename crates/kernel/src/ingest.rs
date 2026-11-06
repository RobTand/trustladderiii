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
        old(w).dependencies@.len() == 0 ==> final(w).nodes@[i as int]
            == port_replies_contracts::ingested(old(w).nodes@[i as int], size, early_end),
        final(w).next_question == old(w).next_question,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds);] {
    kernel_snapshot! { before_ingest = *w }
    finish_ingest_node(&mut w.nodes[i], size, early_end);
    kernel_proof! { port_replies_frames::empty_prerequisites(*w, i as int); }
    w.withhold_dependency_credit(i);
    kernel_proof! {
        assert forall|j: int| 0 <= j < before_ingest@.nodes@.len() && j != i implies
            w.nodes@[j] == before_ingest@.nodes@[j] by {}
    }
    kernel_proof! {
        assert(before_ingest@.dependencies@.len() == 0 ==> w.nodes@[i as int]
            == port_replies_contracts::ingested(before_ingest@.nodes@[i as int], size, early_end)) by {
            reveal(port_replies_contracts::ingested);
        }
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
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn reply_ingest(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        matches!(reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
            | Reply::AuditAgrees | Reply::AuditDisagrees | Reply::ProtectedPaths),
    ensures port_t860_contracts::transition_post(*old(w), *final(w), i as int, reply, *old(out), *final(out)),
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
        ((old(w).dependencies@.len() == 0 || !matches!(reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)))
            && port_replies_contracts::ingest_accepted(old(w).nodes@[i as int], reply))
            ==> port_replies_contracts::ingest_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
                old(w).next_question, reply),
        final(w).next_question == old(w).next_question,
        reply == Reply::AuditDisagrees ==> *final(out) == *old(out),
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] {
    kernel_snapshot! { ingest_before = *w }
    kernel_snapshot! { ingest_source = w.nodes@[i as int] }
    kernel_snapshot! { ingest_id = w.next_question }
    kernel_snapshot! { ingest_fast = w.fail_fast }
    kernel_snapshot! { ingest_out = *out }
    match reply {
        Reply::IngestDone { size } => {
            finish_ingest(w, i, size, None);
            kernel_proof! {
                port_replies_handlers::reply_output_bound(ingest_before@, *w, i as int,
                    ingest_out@.len, out.len);
            }
            kernel_proof! { if ingest_before@.dependencies@.len() == 0 && ingest_source@.phase == Phase::Ingesting {
                port_replies_handlers::fields_done(ingest_source@, w.nodes@[i as int], ingest_id@, size);
            } }
        }
        Reply::IngestEarlyEnd(e) => {
            finish_ingest(w, i, 1, Some(e));
            kernel_proof! {
                port_replies_handlers::reply_output_bound(ingest_before@, *w, i as int,
                    ingest_out@.len, out.len);
            }
            kernel_proof! { if ingest_before@.dependencies@.len() == 0 && ingest_source@.phase == Phase::Ingesting {
                port_replies_handlers::fields_early(ingest_source@, w.nodes@[i as int], ingest_id@, e);
            } }
        }
        Reply::AuditAgrees => reply_audit_agrees(w, i, out),
        Reply::AuditDisagrees => reply_audit_disagrees(w, i, out),
        Reply::ProtectedPaths => enter_stopping(&mut w.nodes[i], Outcome::NotActionable),
        _ => {}
    }
    kernel_proof! {
        if reply == Reply::ProtectedPaths {
            if port_replies_contracts::ingest_accepted(ingest_source@, reply) {
                port_replies_handlers::fields_protected(ingest_source@, w.nodes@[i as int], ingest_id@);
            }
            assert(reply_bounds(ingest_before@, *w, i as int, ingest_out@.len, out.len)) by {
                reveal(reply_bounds);
            }
        }
        if reply == Reply::AuditDisagrees && ingest_source@.phase == Phase::Auditing {
            port_replies_handlers::audit_t860(ingest_source@, w.nodes@[i as int], ingest_id@, ingest_fast@);
        }
        assert(port_t860_contracts::transition_post(ingest_before@, *w, i as int, reply,
            ingest_out@, *out)) by {
            reveal(port_t860_contracts::transition_post);
        }
    }

}
}

kernel_fn! {
/// Audit reply row / #806: private verbatim branch extraction under the standing rule.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn reply_audit_agrees(w: &mut World, i: usize, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        old(w).nodes@[i as int].phase == Phase::Auditing
            ==> port_replies_contracts::ingest_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            old(w).next_question, Reply::AuditAgrees),
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
        final(w).next_question == old(w).next_question,
        successful_reply_input(old(w).nodes@[i as int], Reply::AuditAgrees)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int]),
] {
    kernel_snapshot! { ingest_before = *w }
    kernel_snapshot! { ingest_source = w.nodes@[i as int] }
    kernel_snapshot! { ingest_id = w.next_question }
    kernel_snapshot! { ingest_fast = w.fail_fast }
    kernel_snapshot! { ingest_out = *out }
    match w.nodes[i].early_end {
            Some(e) => enter_stopping(&mut w.nodes[i], e.outcome()),
            None => {
                notice(out, i, Notice::MeasuresPosted);
                set_phase(&mut w.nodes[i], Phase::Unplanned);
            }
        }
    kernel_proof! {
        assert(reply_bounds(ingest_before@, *w, i as int, ingest_out@.len, out.len)) by {
            reveal(reply_bounds);
        }
    }
    kernel_proof! {
        if ingest_source@.phase == Phase::Auditing {
            match ingest_source@.early_end {
                Some(e) => port_replies_handlers::fields_agreed_stop(ingest_source@, w.nodes@[i as int], ingest_id@, e),
                None => port_replies_handlers::fields_agreed_open(ingest_source@, w.nodes@[i as int], ingest_id@),
            }
        }
    }
    kernel_proof! {
        if successful_reply_input(ingest_source@, Reply::AuditAgrees) {
            port_replies_handlers::audit_progress(ingest_source@, w.nodes@[i as int], ingest_id@, ingest_fast@);
        }
    }
}
}

kernel_fn! {
/// Audit reply row / #806: private verbatim branch extraction under the standing rule.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn reply_audit_disagrees(w: &mut World, i: usize, _out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(_out).len, final(_out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        old(w).nodes@[i as int].phase == Phase::Auditing
            ==> port_replies_contracts::ingest_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            old(w).next_question, Reply::AuditDisagrees),
        (old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1 ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
            }),
        retirement::notices_only(*old(_out)) ==> retirement::notices_only(*final(_out)),
        final(w).next_question == old(w).next_question,
        *final(_out) == *old(_out),
        final(w).file_sets@ == old(w).file_sets@,
] {
    kernel_snapshot! { ingest_before = *w }
    kernel_snapshot! { ingest_source = w.nodes@[i as int] }
    kernel_snapshot! { ingest_id = w.next_question }
    kernel_snapshot! { ingest_out = *_out }
            let m = &mut w.nodes[i];
            increment_audit(m);
            kernel_snapshot! { audit_incremented = *m }
            kernel_proof! {
                assert((m.audit_rounds < 2) == (ingest_source@.audit_rounds == 0));
            }
            if m.audit_rounds < 2 {
                // Ingest revises once with the audit's objections.
                set_phase(m, Phase::Ingesting);
                kernel_proof! {
                    assert(reply_bounds(ingest_before@, *w, i as int, ingest_out@.len, _out.len)) by {
                        reveal(reply_bounds);
                    }
                }
                kernel_proof! { if ingest_source@.phase == Phase::Auditing {
                    port_replies_handlers::fields_audit_first(ingest_source@,
                        w.nodes@[i as int], ingest_id@, audit_incremented@);
                } }
            } else {
                // Q27, #832-A: intake cannot split. The one repair is spent.
                stop_for_review(m, ReviewStop::AuditDisagreement);
                kernel_proof! {
                    assert(reply_bounds(ingest_before@, *w, i as int, ingest_out@.len, _out.len)) by {
                        reveal(reply_bounds);
                    }
                }
                kernel_proof! { if ingest_source@.phase == Phase::Auditing {
                    port_replies_handlers::fields_audit_stop(ingest_source@,
                        w.nodes@[i as int], ingest_id@, audit_incremented@);
                } }
            }

}
}

#[cfg(test)]
#[path = "ingest_equivalence/mod.rs"]
mod reply_ingest_equivalence;

kernel_fn! {
/// Audit disagreement / #806: verbatim saturation isolated from world frame proofs.
fn increment_audit(m: &mut Node) -> (result: ()) [
    ensures *final(m) == (Node { audit_rounds: if old(m).audit_rounds < u8::MAX {
        (old(m).audit_rounds + 1) as u8
    } else { u8::MAX }, ..*old(m) }),
] {
    m.audit_rounds = m.audit_rounds.saturating_add(1);
}
}
