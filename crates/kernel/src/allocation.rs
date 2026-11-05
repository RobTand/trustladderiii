//! Executable allocation transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
    /// Every phase change starts a new generation, and the node stops waiting on
    /// whatever effect it had in flight. #771: one generation increment.
    pub(super) fn set_phase(m: &mut Node, phase: Phase) -> (result: ()) [
        requires old(m).generation < u64::MAX,
        ensures
            *final(m) == (Node {
                generation: (old(m).generation + 1) as u64,
                phase, pos: 0, ticket: None,
                effect: match old(m).effect {
                    Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                },
                ..*old(m)
            }),
            final(m).generation as int == old(m).generation as int + 1,
            final(m).phase == phase, final(m).pos == 0, final(m).ticket.is_none(),
            final(m).effect == match old(m).effect {
                Some(e) => Some(InFlight { cancelling: true, ..e }),
                None => None,
            },
            final(m).credit == old(m).credit, final(m).next_seq == old(m).next_seq,
            final(m).last_cmd == old(m).last_cmd, final(m).question == old(m).question,
            final(m).run == old(m).run, final(m).job == old(m).job,
            final(m).pool == old(m).pool, final(m).holds == old(m).holds,
            final(m).paused == old(m).paused, final(m).waiting_incident == old(m).waiting_incident,
    ] {
        if let Some(e) = m.effect.as_mut() {
            e.cancelling = true;
        }
        m.phase = phase;
        m.pos = 0;
        m.ticket = None;
        m.generation += 1;
    }
}

kernel_fn! {
/// Q30 / #771: releasing an operation preserves all allocation counters.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
pub(super) fn abandon_operation(m: &mut Node) -> (result: ()) [
    ensures *final(m) == (Node {
        run: None, job: None, pool: false,
        effect: match old(m).effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || matches!(e.kind, EffectKind::StartRun(_)
                    | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
            None => None,
        },
        ..*old(m)
    }),
] {
    m.run = None;
    m.job = None;
    m.pool = false;
    if let Some(e) = m.effect.as_mut() {
        if e.kind.starts_operation() {
            e.cancelling = true;
        }
    }
}
}

kernel_fn! {
/// #771: successful completion resets failures and preserves allocation counters.
pub(super) fn end_operation(m: &mut Node) -> (result: ()) [
    ensures *final(m) == (Node { run: None, job: None, pool: false,
        run_failures: 0, run_incidents: 0,
        effect: match old(m).effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || matches!(e.kind, EffectKind::StartRun(_)
                    | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
            None => None,
        }, ..*old(m) }),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).run.is_none(), final(m).job.is_none(), !final(m).pool,
        final(m).run_failures == 0, final(m).run_incidents == 0,
        final(m).phase == old(m).phase, final(m).pos == old(m).pos,
        final(m).generation == old(m).generation, final(m).next_seq == old(m).next_seq,
        final(m).credit == old(m).credit, final(m).question == old(m).question,
        final(m).ticket == old(m).ticket,
        final(m).footprint == old(m).footprint,
        final(m).last_cmd == old(m).last_cmd,
        final(m).fx_failures == old(m).fx_failures,
        final(m).fx_incidents == old(m).fx_incidents,

] {
    abandon_operation(m);
    m.run_failures = 0;
    m.run_incidents = 0;
}
}

kernel_fn! {
/// Stop and replacement rows / #771: at most one generation is allocated.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn enter_stopping(m: &mut Node, outcome: Outcome) -> (result: ()) [
    requires !(old(m).phase is Closed || old(m).phase is Stopping)
        ==> old(m).generation < u64::MAX,
    ensures *final(m) == retirement::stopped(*old(m), outcome),
        (!old(m).paused && !old(m).waiting_incident && old(m).effect.is_none()
            && !(old(m).phase is Closed || old(m).phase is Stopping)
            ==> distance(*final(m)) < distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int == old(m).generation as int
            + if old(m).phase is Closed || old(m).phase is Stopping { 0int } else { 1int },
        final(m).phase == match old(m).phase {
            Phase::Closed(o) => Phase::Closed(o),
            Phase::Stopping(o) => Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }),
            _ => Phase::Stopping(outcome),
        },
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd,
        final(m).review_stop == old(m).review_stop,
        (old(m).phase is Closed || old(m).phase is Stopping) ==> *final(m) == (Node {
            phase: final(m).phase, ..*old(m)
        }),
        !(old(m).phase is Closed || old(m).phase is Stopping) ==>
            final(m).pos == 0 && final(m).ticket.is_none() && final(m).question.is_none()
                && !final(m).waiting_incident && !final(m).incident_needs_rob,
] proof [reveal(retirement::stopped);] {
    kernel_snapshot! { before = *m }
    match m.phase {
        Phase::Closed(_) => {}
        Phase::Stopping(_) => {
            if outcome == Outcome::Merged {
                m.phase = Phase::Stopping(Outcome::Merged);
            }
        }
        _ => {
            m.question = None;
            m.waiting_incident = false;
            m.incident_needs_rob = false;
            set_phase(m, Phase::Stopping(outcome));
        }
    }
    kernel_proof! {
        allocation_stopping_lemmas::stopping_frame(before@, *m, outcome);
        allocation_stopping_lemmas::stopping_progress(before@, *m, outcome);
    }
}
}

kernel_fn! {
    /// Q18 / #771: merge credit is allocated once, increasing earned by at most one.
    pub(super) fn merge_credit(m: &mut Node) -> (result: ()) [
        requires old(m).credit.merge_credited || old(m).credit.earned < u64::MAX,
        ensures
            *final(m) == (Node { credit: Credit {
                earned: final(m).credit.earned, merge_credited: true, ..old(m).credit
            }, ..*old(m) }),
            final(m).credit.earned as int == old(m).credit.earned as int
                + if old(m).credit.merge_credited { 0int } else { 1int },
            final(m).credit.merge_credited,
            final(m).credit.start == old(m).credit.start,
            final(m).credit.added == old(m).credit.added,
            final(m).credit.spent == old(m).credit.spent,
            final(m).credit.ready == old(m).credit.ready,
            final(m).credit.withheld == old(m).credit.withheld,
            final(m).generation == old(m).generation, final(m).next_seq == old(m).next_seq,
            final(m).phase == old(m).phase, final(m).pos == old(m).pos,
            final(m).effect == old(m).effect, final(m).question == old(m).question,
        final(m).ticket == old(m).ticket,
            final(m).paused == old(m).paused,
            final(m).waiting_incident == old(m).waiting_incident,
    ] {
        if !m.credit.merge_credited {
            m.credit.earned += 1;
            m.credit.merge_credited = true;
        }
    }
}

kernel_fn! {
    pub(super) fn close_node(m: &mut Node, i: usize, outcome: Outcome, out: &mut Outbox) -> (result: ()) [
    requires old(m).credit.merge_credited || old(m).credit.earned < u64::MAX,
        old(out).len + 2 <= OUTBOX,
    ensures final(m).phase == Phase::Closed(outcome), final(m).pos == 0,
        *final(m) == (Node { phase: Phase::Closed(outcome), pos: 0, ticket: None,
            credit: Credit { earned: final(m).credit.earned,
                merge_credited: final(m).credit.merge_credited, ..old(m).credit }, ..*old(m) }),
        final(m).ticket.is_none(),
        final(m).generation == old(m).generation, final(m).next_seq == old(m).next_seq,
        final(m).credit.earned as int == old(m).credit.earned as int
            + if outcome == Outcome::Merged && !old(m).credit.merge_credited { 1int } else { 0int },
        final(m).credit.merge_credited == (old(m).credit.merge_credited || outcome == Outcome::Merged),
        final(m).credit.start == old(m).credit.start, final(m).credit.added == old(m).credit.added,
        final(m).credit.spent == old(m).credit.spent,
        final(out).len <= old(out).len + 2,
    ] {
        m.phase = Phase::Closed(outcome);
        m.pos = 0;
        m.ticket = None;
        if outcome == Outcome::Merged {
            merge_credit(m);
        }
        notice(out, i, Notice::Summary(outcome));
        if outcome == Outcome::Replaced {
            notice(out, i, Notice::FileReplacement);
        }
    }
}

kernel_fn! {
/// Question rows / #771: one question and one generation, on the selected node.
pub(super) fn ask_rob(w: &mut World, i: usize, kind: QKind, cont: Cont, cost: u8,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].generation < u64::MAX, old(out).len < OUTBOX,
    ensures final(w).next_question as int == old(w).next_question as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int] == (Node {
            holds: false,
            question: Some(Question { id: old(w).next_question, kind, cont, cost, clarified: false }),
            phase: Phase::AwaitingRob,
            generation: (old(w).nodes@[i as int].generation + 1) as u64,
            pos: 0, ticket: None,
            effect: match old(w).nodes@[i as int].effect {
                Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
            },
            ..old(w).nodes@[i as int]
        }),
        final(out).len == old(out).len + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let id = w.next_question;
    w.next_question += 1;
    let m = &mut w.nodes[i];
    m.holds = false;
    m.question = Some(Question {
        id,
        kind,
        cont,
        cost,
        clarified: false,
    });
    set_phase(m, Phase::AwaitingRob);
    notice(out, i, Notice::Ask { question: id });
}
}

kernel_fn! {
/// Incident rows / #771: only a holding phase allocates a generation.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn raise_incident(w: &mut World, i: usize, needs_rob: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        holding(old(w).nodes@[i as int].phase) ==>
            old(w).nodes@[i as int].generation < u64::MAX,
    ensures final(w).nodes@[i as int] == retirement::incident(old(w).nodes@[i as int], needs_rob),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int
            == old(w).nodes@[i as int].generation as int
                + if holding(old(w).nodes@[i as int].phase) { 1int } else { 0int },
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].waiting_incident,
        final(w).nodes@[i as int].incident_needs_rob == needs_rob,
        final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].run_failures == old(w).nodes@[i as int].run_failures,
        final(w).nodes@[i as int].run_incidents == old(w).nodes@[i as int].run_incidents,
        final(w).nodes@[i as int].fx_failures == old(w).nodes@[i as int].fx_failures,
        final(w).nodes@[i as int].fx_incidents == old(w).nodes@[i as int].fx_incidents,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 1,
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    kernel_snapshot! { before_incident = w.nodes@[i as int] }
    let m = &mut w.nodes[i];
    m.waiting_incident = true;
    m.incident_needs_rob = needs_rob;
    m.ticket = None;
    // A leaf waiting on an incident gives back its slot and files.
    if let Some(r) = m.phase.resume() {
        m.holds = false;
        set_phase(m, Phase::Ready(r));
        kernel_proof! {
            allocation_incident_lemmas::resumed_result(before_incident@, *m, needs_rob, r);
        }
    }
    kernel_proof! {
        if !holding(before_incident@.phase) {
            allocation_incident_lemmas::waiting_result(before_incident@, w.nodes@[i as int], needs_rob);
        }
    }
    notice(out, i, Notice::Incident { needs_rob });
    kernel_proof! {
        allocation_incident_lemmas::incident_frame(before_incident@, w.nodes@[i as int], needs_rob);
    }
}
}

kernel_fn! {
/// #771: the world wrapper emits on exactly one selected node.
pub(super) fn emit(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].next_seq < u64::MAX,
    ensures final(w).nodes@[i as int] == (Node {
        next_seq: (old(w).nodes@[i as int].next_seq + 1) as u64,
        effect: Some(InFlight { seq: old(w).nodes@[i as int].next_seq, kind, cancelling: false }),
        worktree: old(w).nodes@[i as int].worktree || kind == EffectKind::PrepareWorktree,
        pr_open: old(w).nodes@[i as int].pr_open || kind == EffectKind::OpenPr,
        had_pr: old(w).nodes@[i as int].had_pr || kind == EffectKind::OpenPr,
        ..old(w).nodes@[i as int]
    }),
        final(w).next_ticket == old(w).next_ticket,
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    emit_node(&mut w.nodes[i], i, kind, out);
}
}

kernel_fn! {
    /// #771: effect allocation increases next_seq by exactly one.
    pub(super) fn emit_node(m: &mut Node, i: usize, kind: EffectKind, out: &mut Outbox) -> (result: ()) [
    requires old(m).next_seq < u64::MAX, old(out).len < OUTBOX,
    ensures *final(m) == (Node {
        next_seq: (old(m).next_seq + 1) as u64,
        effect: Some(InFlight { seq: old(m).next_seq, kind, cancelling: false }),
        worktree: old(m).worktree || kind == EffectKind::PrepareWorktree,
        pr_open: old(m).pr_open || kind == EffectKind::OpenPr,
        had_pr: old(m).had_pr || kind == EffectKind::OpenPr,
        ..*old(m)
    }),
    final(out).len == old(out).len + 1,
    ] {
        let seq = m.next_seq;
        m.next_seq += 1;
        m.effect = Some(InFlight {
            seq,
            kind,
            cancelling: false,
        });
        match kind {
            EffectKind::PrepareWorktree => m.worktree = true,
            EffectKind::OpenPr => {
                m.pr_open = true;
                m.had_pr = true;
            }
            _ => {}
        }
        let head = match kind {
            EffectKind::Push | EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::Enqueue => {
                m.head
            }
            _ => None,
        };
        out.push(Out::Effect {
            node: NodeId(i as u64),
            seq,
            kind,
            head,
        });
    }
}
