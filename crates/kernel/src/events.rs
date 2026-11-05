//! The executable event dispatcher, handlers and allocation primitives.
use super::*;

kernel_fn! {
/// #771: compose the actual event handlers' per-apply counter bounds.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn apply(w: &mut World, event: Event, out: &mut Outbox) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures forall|j: int| 0 <= j < final(w).nodes@.len() ==>
            settle_shape(final(w).nodes@[j])
                && footprint::valid_footprint(*final(w), final(w).nodes@[j].footprint),
        forall|j: int| 0 <= j < old(w).nodes@.len() ==>
            (old(w).nodes@[j].phase is Closed ==> final(w).nodes@[j].phase is Closed)
                && final(w).nodes@[j].repo == old(w).nodes@[j].repo
                && final(w).nodes@[j].last_cmd >= old(w).nodes@[j].last_cmd,
        old(w).nodes@.len() <= final(w).nodes@.len() <= old(w).nodes@.len() + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@, final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        forall|j: int| 0 <= j < old(w).nodes@.len() ==>
            final(w).nodes@[j].generation as int <= old(w).nodes@[j].generation as int + 2
            && final(w).nodes@[j].next_seq as int <= old(w).nodes@[j].next_seq as int + 1
            && final(w).nodes@[j].credit.start as int <= old(w).nodes@[j].credit.start as int + CREDIT_START_STEP_BUDGET
            && final(w).nodes@[j].credit.earned as int + merge_capacity(final(w).nodes@[j])
                <= old(w).nodes@[j].credit.earned as int + 1
            && final(w).nodes@[j].credit.added as int <= old(w).nodes@[j].credit.added as int + CREDIT_ADDED_STEP_BUDGET
            && final(w).nodes@[j].credit.spent as int <= old(w).nodes@[j].credit.spent as int + CREDIT_SPENT_STEP_BUDGET,
        forall|j: int| old(w).nodes@.len() <= j < final(w).nodes@.len() ==>
            final(w).nodes@[j].last_cmd == 0
            && final(w).nodes@[j].generation == 0 && final(w).nodes@[j].next_seq == 0
            && final(w).nodes@[j].credit.start == 0 && final(w).nodes@[j].credit.earned == 0
            && final(w).nodes@[j].credit.added == 0 && final(w).nodes@[j].credit.spent == 0,
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_apply = *w }
    match event {
        Event::NewAdoption {
            repo,
            head,
            priority,
        } => {
            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
                let mut node = Node::new(repo);
                node.proposed = Some(head);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::NewRequest { repo, priority } => {
            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
                let mut node = Node::new(repo);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::Reply {
            node,
            generation,
            reply,
        } => {
            if let Some(i) = w.index(node) {
                on_reply(w, i, generation, reply, out);
            }
        }
        Event::Ack { node, seq, ack } => {
            if let Some(i) = w.index(node) {
                on_ack(w, i, seq, ack, out);
            }
        }
        Event::Fact { node, fact } => {
            if let Some(i) = w.index(node) {
                on_fact(w, i, fact, out);
            }
        }
        Event::Command { node, id, command } => {
            if let Some(i) = w.index(node) {
                on_command(w, i, id, command, out);
            }
        }
    }
    kernel_proof! {
        assert forall|j: int| 0 <= j < w.nodes@.len() implies
            footprint::valid_footprint(*w, w.nodes@[j].footprint) by {
            if j < before_apply@.nodes@.len() {
                if w.nodes@[j].footprint == before_apply@.nodes@[j].footprint {
                    footprint::valid_under_table_growth(before_apply@, *w, w.nodes@[j].footprint);
                }
            } else {
                footprint::inline_is_valid(*w, 0);
            }
        }
    }
}
}

// ---------------------------------------------------------------- transitions

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
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
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
pub(super) fn enter_stopping(m: &mut Node, outcome: Outcome) -> (result: ()) [
    requires !(old(m).phase is Closed || old(m).phase is Stopping)
        ==> old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
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
] {
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
}
}

kernel_fn! {
    /// Q18 / #771: merge credit is allocated once, increasing earned by at most one.
    pub(super) fn merge_credit(m: &mut Node) -> (result: ()) [
        requires old(m).credit.merge_credited || old(m).credit.earned < u32::MAX,
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
    requires old(m).credit.merge_credited || old(m).credit.earned < u32::MAX,
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
        final(w).nodes@ == old(w).nodes@.update(i as int, Node {
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
pub(super) fn raise_incident(w: &mut World, i: usize, needs_rob: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        holding(old(w).nodes@[i as int].phase) ==>
            old(w).nodes@[i as int].generation < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
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
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let m = &mut w.nodes[i];
    m.waiting_incident = true;
    m.incident_needs_rob = needs_rob;
    m.ticket = None;
    // A leaf waiting on an incident gives back its slot and files.
    if let Some(r) = m.phase.resume() {
        m.holds = false;
        set_phase(m, Phase::Ready(r));
    }
    notice(out, i, Notice::Incident { needs_rob });
}
}

kernel_fn! {
/// #771: the world wrapper emits on exactly one selected node.
pub(super) fn emit(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].next_seq < u64::MAX,
    ensures final(w).nodes@ == old(w).nodes@.update(i as int, Node {
        next_seq: (old(w).nodes@[i as int].next_seq + 1) as u64,
        effect: Some(InFlight { seq: old(w).nodes@[i as int].next_seq, kind, cancelling: false }),
        worktree: old(w).nodes@[i as int].worktree || kind == EffectKind::PrepareWorktree,
        pr_open: old(w).nodes@[i as int].pr_open || kind == EffectKind::OpenPr,
        had_pr: old(w).nodes@[i as int].had_pr || kind == EffectKind::OpenPr,
        ..old(w).nodes@[i as int]
    }),
        final(w).next_ticket == old(w).next_ticket,
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

// ---------------------------------------------------------------- replies

kernel_fn! {
/// Reply rows / #771: phase, schema and commit identity use the executable checks.
pub(super) fn accepts(w: &World, n: &Node, reply: &Reply) -> (result: bool) [
    ensures result == match (n.phase, *reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) =>
            footprint::valid_footprint(*w, footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) =>
            footprint::valid_footprint(*w, files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        (Phase::Checking, Reply::ChecksDone { report, .. }) => report,
        (Phase::Reviewing, Reply::Approved { head }) =>
            n.head == Some(head) && n.checked == Some(head),
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges) => true,
        _ => false,
    },
] {
    match (n.phase, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => {
            true
        }
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) => w.has_footprint(*footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) => w.has_footprint(*files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        // No report means the tests never ran: an infrastructure failure.
        (Phase::Checking, Reply::ChecksDone { report, .. }) => *report,
        (Phase::Reviewing, Reply::Approved { head }) => {
            n.head == Some(*head) && n.checked == Some(*head)
        }
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. }) => true,
        (Phase::Rebasing, Reply::RebaseNeedsChanges) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Reply rows / #771: stale operations stutter, invalid replies use the bounded
/// failure handler, and successful replies use the actual transition bodies.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn on_reply(w: &mut World, i: usize, generation: u64, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start as int <= old(w).nodes@[i as int].credit.start as int + CREDIT_START_STEP_BUDGET,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(reply_bounds); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_reply = *w }
    let n = &w.nodes[i];
    if n.phase.is_stopping() {
        // The run or job ended on its own, so teardown needn't stop it.
        let m = &mut w.nodes[i];
        if m.run == Some(generation) {
            m.run = None;
            m.pool = false;
        }
        if m.job == Some(generation) {
            m.job = None;
        }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        return;
    }
    if n.current() != Some(Item::Await) || (n.run != Some(generation) && n.job != Some(generation))
    {
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        return; // stale
    }
    // A reply that breaks its schema or names the wrong commit counts as a failed run.
    if !accepts(w, n, &reply) {
        run_failed(w, i, out);
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        return;
    }
    kernel_snapshot! { before_operation = *w }
    end_operation(&mut w.nodes[i]);
    kernel_proof! {
        footprint::valid_under_table_growth(before_operation@, *w, w.nodes@[i as int].footprint);
    }
    match reply {
        Reply::MakerNeedsFiles { files, fits } => {
            kernel_proof! { footprint::valid_under_table_growth(before_operation@, *w, files); }
            reply_files(w, i, files, fits, out);
        }
        _ => {
            reply_transition(w, i, reply, out);
            kernel_proof! {
                if let Reply::PlanLeaf { footprint, .. } = reply {
                    footprint::valid_under_table_growth(before_operation@, *w, footprint);
                }
                footprint::valid_under_table_growth(before_operation@, *w, before_operation@.nodes@[i as int].footprint);
            }
        }
    }
}
}

kernel_fn! {
/// File extension rows / #771: union only appends file storage; the transition
/// spends at most one credit and allocates one generation or question.
pub(super) fn reply_files(w: &mut World, i: usize, files: Footprint, fits: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::valid_footprint(*old(w), files), footprint::ordered_table(*old(w)),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_files = *w }
    if fits && !files.is_empty() && w.nodes[i].credit.can_spend(1) {
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        kernel_snapshot! { after_union = *w }
        let m = &mut w.nodes[i];
        m.credit.spend(1);
        m.footprint = footprint;
        m.holds = false;
        set_phase(m, Phase::Ready(Resume::Making));
        kernel_proof! { footprint::valid_under_table_growth(after_union@, *w, footprint); }
    } else if fits && !files.is_empty() && w.fail_fast {
        // Q54 stops retries, not credit decisions: Rob funds the extension.
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        kernel_snapshot! { after_union = *w }
        w.nodes[i].footprint = footprint;
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(Resume::Making),
            1,
            out,
        );
        kernel_proof! { footprint::valid_under_table_growth(after_union@, *w, footprint); }
    } else {
        out_of_room(w, i, ReviewStop::MakerNeedsFiles, out);
        kernel_proof! { footprint::valid_under_table_growth(before_files@, *w, w.nodes@[i as int].footprint); }
    }
    }
}

kernel_fn! {
/// Reply rows / #771: partition the unchanged match for bounded solver queries.
pub(super) fn reply_transition(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    match reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees
            | Reply::AuditDisagrees | Reply::ProtectedPaths => reply_ingest(w, i, reply, out),
        Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet => reply_plan(w, i, reply, out),
        _ => reply_work(w, i, reply, out),
    }
}
}

kernel_fn! {
/// Ingest reply rows / #771: initialize the same one-time deposit before audit.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn finish_ingest(w: &mut World, i: usize, size: u8, early_end: Option<EarlyEnd>)
    -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, 0, 0),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(reply_bounds);] {
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
            start: if old(m).credit.ready { old(m).credit.start } else { size as u32 + RESERVE },
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
        m.credit.start = size as u32 + RESERVE;
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
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
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

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
pub(super) fn reply_plan(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::PlanLeaf { footprint, fits } => {
            let m = &mut w.nodes[i];
            let must_split = matches!(m.phase, Phase::Planning { must_split: true });
            if fits && !footprint.is_empty() && !must_split {
                m.footprint = footprint;
                m.attempt = 1;
                m.review_restarted = false;
                m.round = 0;
                m.plan_retries = 0;
                set_phase(m, Phase::Ready(Resume::Making));
            } else {
                plan_retry(w, i, false, out);
            }
        }
        Reply::PlanSplit => ask_rob(w, i, QKind::NeedsSplitting, Cont::Nowhere, 0, out),
        Reply::PlanCantMeet => plan_retry(w, i, true, out),
        _ => {}
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn reply_work(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::MakerDone { head } => {
            let m = &mut w.nodes[i];
            m.head = Some(head);
            m.checked = None;
            set_phase(m, Phase::Checking);
        }
        Reply::MakerCantMeet => next_rung(w, i, ReviewStop::MakerCantMeet, out),
        Reply::MakerOutOfRoom => out_of_room(w, i, ReviewStop::MakerOutOfRoom, out),
        Reply::ChecksDone {
            pass, in_footprint, ..
        } => {
            if pass && in_footprint {
                let m = &mut w.nodes[i];
                m.checked = m.head;
                set_phase(m, Phase::Reviewing);
            } else {
                failed_round(w, i, ReviewStop::ChecksFailed, out);
            }
        }
        Reply::Approved { head } => {
            let m = &mut w.nodes[i];
            m.approved = Some(head);
            set_phase(m, Phase::Publishing);
        }
        Reply::ChangesRequested => failed_round(w, i, ReviewStop::ChangesRequested, out),
        Reply::ReviewerOutOfRoom => {
            if w.nodes[i].review_restarted {
                out_of_room(w, i, ReviewStop::ReviewerOutOfRoom, out);
            } else {
                w.nodes[i].review_restarted = true;
                recheck(w, i, Phase::Reviewing, out);
            }
        }
        _ => reply_rebase(w, i, reply, out),
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
pub(super) fn reply_rebase(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::RebaseClean { head } => {
            let m = &mut w.nodes[i];
            if m.head != Some(head) {
                m.review_restarted = false;
            }
            m.head = Some(head);
            m.checked = None;
            recheck(w, i, Phase::Checking, out);
        }
        Reply::RebaseNeedsChanges => {
            let m = &mut w.nodes[i];
            if m.credit.can_spend(1) {
                m.credit.spend(1);
                set_phase(m, Phase::Making);
            } else {
                ask_rob(
                    w,
                    i,
                    QKind::OutOfCredit,
                    Cont::Ready(Resume::Making),
                    1,
                    out,
                );
            }
        }
        _ => {}
    }
}
}

kernel_fn! {
/// Failed-run rows / #771: a retry and incident allocate at most two generations.
pub(super) fn run_failed(w: &mut World, i: usize, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].run_failures <= 1,
        old(w).nodes@[i as int].run_incidents <= 2,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].run_failures <= 1,
        final(w).nodes@[i as int].run_incidents <= 2,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let m = &mut w.nodes[i];
    abandon_operation(m);
    m.pos = operation_start(m.phase);
    m.generation += 1;
    m.run_failures += 1;
    if m.run_failures >= 2 {
        // Every second failure raises an incident. Only Rob can clear the second one.
        m.run_failures = 0;
        m.run_incidents = (m.run_incidents + 1).min(2);
        let needs_rob = m.run_incidents >= 2;
        raise_incident(w, i, needs_rob, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: stopping for review spends no credit and allocates at most one generation.
/// Under `fail_fast`, a step that would retry the work stops for TL3 review.
/// It asks Rob nothing; the summary offers `restart`, as for any stop (Q49).
pub(super) fn stop_for_review(m: &mut Node, step: ReviewStop) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd, final(m).review_stop == Some(step),
] {
    m.review_stop = Some(step);
    enter_stopping(m, Outcome::Stopped);
}
}

kernel_fn! {
/// Failed-round rows / #771: the free fix and paid ladder share one-step bounds.
pub(super) fn failed_round(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    let m = &mut w.nodes[i];
    if m.round == 0 {
        // The fix round resumes the maker's session. It's free.
        m.round = 1;
        set_phase(m, Phase::Making);
    } else {
        next_rung(w, i, step, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: the ladder after a failed fix round costs at most one credit.
/// It tries a bigger budget, another model, one re-plan, then asks Rob.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn next_rung(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
    } else if m.attempt < 3 {
        m.attempt += 1;
        m.review_restarted = false;
        m.round = 0;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            set_phase(m, Phase::Making);
        } else {
            ask_rob(
                w,
                i,
                QKind::OutOfCredit,
                Cont::Ready(Resume::Making),
                1,
                out,
            );
        }
    } else if !m.replanned {
        m.replanned = true;
        m.holds = false;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split: false });
            notice(out, i, Notice::Note(NoteKind::Replanned));
        } else {
            ask_rob(
                w,
                i,
                QKind::OutOfCredit,
                Cont::Planning { must_split: false },
                1,
                out,
            );
        }
    } else {
        ask_rob(w, i, QKind::CantMeet, Cont::Ready(Resume::Making), 1, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: exhaustion either stops, pays once to re-plan, or asks Rob.
pub(super) fn out_of_room(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
        return;
    }
    m.holds = false;
    if m.credit.can_spend(1) {
        m.credit.spend(1);
        m.plan_retries = 0;
        set_phase(m, Phase::Planning { must_split: true });
    } else {
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Planning { must_split: true },
            1,
            out,
        );
    }
}
}

kernel_fn! {
/// Q54 / #771: planner retries saturate their local retry count.
/// Under `fail_fast`, `cant_meet` stops for review. A plan that does not fit
/// keeps the existing retry rule; the live worker reports broken rules as `cant_meet`.
pub(super) fn plan_retry(w: &mut World, i: usize, cant_meet: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    m.plan_retries = m.plan_retries.saturating_add(1);
    let phase = m.phase;
    if fail_fast && cant_meet {
        stop_for_review(m, ReviewStop::PlanCantMeet);
    } else if m.plan_retries < 2 {
        // Re-run once, with the reason or the next model on the planner's ladder.
        set_phase(m, phase);
    } else {
        m.plan_retries = 0;
        let must_split = matches!(phase, Phase::Planning { must_split: true });
        ask_rob(w, i, QKind::CantMeet, Cont::Planning { must_split }, 1, out);
    }
}
}

kernel_fn! {
/// Recheck rows / #771: a new head or reviewer uses the same free allowance.
pub(super) fn recheck(w: &mut World, i: usize, target: Phase,
    out: &mut Outbox) -> (result: ()) [
    requires !(target is Closed), i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    let m = &mut w.nodes[i];
    m.rechecks = m.rechecks.saturating_add(1);
    let free = m.rechecks <= FREE_RECHECKS;
    if free || m.credit.can_spend(1) {
        if !free {
            m.credit.spend(1);
        }
        if !target.holding() {
            m.holds = false;
        }
        set_phase(m, target);
    } else {
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(if target == Phase::Reviewing {
                Resume::Reviewing
            } else {
                Resume::Checking
            }),
            1,
            out,
        );
    }
}
}

// ---------------------------------------------------------------- acknowledgements

kernel_fn! {
/// Acknowledgement rows / #771: stale delivery keys stutter; retries stay bounded.
pub(super) fn on_ack(w: &mut World, i: usize, seq: u64, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed && old(w).nodes@[i as int].effect.is_none()
            ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) && stopping_effect_fits(old(w).nodes@[i as int])
            ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq as int <= old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let Some(e) = w.nodes[i].effect else { return };
    if e.seq != seq {
        return;
    }
    w.nodes[i].effect = None;
    if e.cancelling {
        return;
    }
    match ack {
        Ack::Confirmed => confirmed(&mut w.nodes[i], e.kind),
        Ack::Retryable => delivery_failed(w, i, e.kind, out),
        Ack::Permanent => effect_failed(w, i, e.kind, out),
        Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PublicationBody);
        }
        Ack::BodyPolicyRefused => effect_failed(w, i, e.kind, out),
    }
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: at most one adoption generation is allocated.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn confirmed(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m))
            && (old(m).phase is Stopping ==> current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).generation >= old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).fx_failures == 0, final(m).fx_incidents == 0,
        final(m).last_cmd == old(m).last_cmd,
] {
    m.fx_failures = 0;
    m.fx_incidents = 0;
    match kind {
        EffectKind::StopRun => {
            m.run = None;
            m.pool = false;
        }
        EffectKind::WithdrawJob => m.job = None,
        EffectKind::ClosePr => m.pr_open = false,
        EffectKind::RemoveWorktree => m.worktree = false,
        _ => {}
    }
    if !kind.starts_operation() {
        if let Some(Item::Do(k) | Item::Teardown(k)) = m.current() {
            if k == kind {
                if kind == EffectKind::PrepareWorktree && !m.adopted {
                    if let Some(head) = m.proposed {
                        m.adopted = true;
                        m.head = Some(head);
                        m.checked = None;
                        set_phase(m, Phase::Checking);
                        return;
                    }
                }
                m.pos += 1;
            }
        }
    }
}
}

kernel_fn! {
/// Failed-delivery rows / #771: at most two generations or one retried effect.
pub(super) fn delivery_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].next_seq < u64::MAX,
        old(w).nodes@[i as int].fx_failures <= 1,
        old(w).nodes@[i as int].fx_incidents <= 2,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq as int <= old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].fx_failures <= 1,
        final(w).nodes@[i as int].fx_incidents <= 2,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let m = &mut w.nodes[i];
    m.fx_failures += 1;
    if m.fx_failures < 2 {
        emit(w, i, kind, out);
        return;
    }
    m.fx_failures = 0;
    m.fx_incidents = (m.fx_incidents + 1).min(2);
    let needs_rob = m.fx_incidents >= 2;
    if kind.starts_operation() {
        abandon_operation(m);
        m.pos = operation_start(m.phase);
        m.generation += 1;
    }
    raise_incident(w, i, needs_rob, out);
}
}

kernel_fn! {
/// Permanent-effect-failure rows / #771: exactly one question, no credit change.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn effect_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int == old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 2,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    notice(out, i, Notice::Incident { needs_rob: true });
    if w.nodes[i].phase.is_stopping() {
        // Cleanup stays put, keeping what it holds, until Rob retries or marks the step handled.
        let id = w.next_question;
        w.next_question += 1;
        w.nodes[i].question = Some(Question {
            id,
            kind: QKind::CleanupFailed,
            cont: Cont::Nowhere,
            cost: 0,
            clarified: false,
        });
        notice(out, i, Notice::Ask { question: id });
        return;
    }
    let m = &mut w.nodes[i];
    if kind.starts_operation() {
        abandon_operation(m);
    }
    let cont = match m.phase {
        Phase::Ingesting => Cont::Ingesting,
        Phase::Auditing => Cont::Auditing,
        Phase::Planning { must_split } => Cont::Planning { must_split },
        phase => match phase.resume() {
            Some(r) => Cont::Ready(r),
            None => Cont::Unplanned,
        },
    };
    ask_rob(w, i, QKind::EffectFailed, cont, 0, out);
}
}

// ---------------------------------------------------------------- facts

kernel_fn! {
/// Fact rows / #771: compose declaration updates, merge credit and the remaining
/// fact handlers. No fact assigns a command ID, effect sequence or ticket.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn on_fact(w: &mut World, i: usize, fact: Fact, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        dependencies::edges_indexable(*old(w)), old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned as int + merge_capacity(final(w).nodes@[i as int])
            <= old(w).nodes@[i as int].credit.earned as int + 1,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(out).len <= old(out).len + 4,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    match fact {
        Fact::DependencyRemove { kind, number } => w.remove_dependency(i, kind, number),
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        Fact::PrMerged {
            head,
            checks_passed,
        } => pr_merged(w, i, head, checks_passed, out),
        _ => fact_transition(w, i, fact, out),
    }
}
}

kernel_fn! {
/// Fact rows / #771: non-dependency, non-merge facts allocate at most one
/// generation, one question and one paid credit, and emit at most two notices.
#[allow(clippy::collapsible_if)]
pub(super) fn fact_transition(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(fact is DependencyRemove || fact is Dependency || fact is PrMerged),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(retry_bounds);] {
    match fact {
        Fact::DependencyRemove { .. } | Fact::Dependency { .. } | Fact::PrMerged { .. } => {}
        Fact::Priority(priority) => {
            if !w.nodes[i].phase.is_closed() {
                w.nodes[i].priority = priority;
            }
        }
        Fact::PrClosedByPerson => {
            let m = &mut w.nodes[i];
            if !m.pr_open {
                return;
            }
            m.pr_open = false;
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                notice(out, i, Notice::Note(NoteKind::PrClosed));
                enter_stopping(m, Outcome::Stopped);
            }
        }
        Fact::PrDequeuedByPerson => {
            let m = &mut w.nodes[i];
            if m.phase != Phase::Queued {
                return;
            }
            // Treated as Rob pausing it: the PR waits until he resumes.
            m.holds = false;
            m.paused = true;
            set_phase(m, Phase::Ready(Resume::Publishing));
            notice(out, i, Notice::Note(NoteKind::PrDequeued));
        }
        Fact::PushByOther { head } => {
            let m = &w.nodes[i];
            if !m.had_pr {
                return;
            }
            notice(out, i, Notice::Note(NoteKind::OutsidePush));
            let published = matches!(
                m.phase,
                Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing)
            );
            if published {
                let m = &mut w.nodes[i];
                if m.head != Some(head) {
                    m.review_restarted = false;
                }
                m.head = Some(head);
                m.checked = None;
                recheck(w, i, Phase::Ready(Resume::Checking), out);
            }
        }
        Fact::CiFailed { infra } => {
            let m = &mut w.nodes[i];
            let before_enqueue =
                m.phase == Phase::Publishing && m.current() == Some(Item::Do(EffectKind::Enqueue));
            if m.phase != Phase::Queued && !before_enqueue {
                return;
            }
            if before_enqueue {
                if let Some(effect) = m.effect.as_mut() {
                    effect.cancelling = true;
                }
            }
            if !infra {
                failed_round(w, i, ReviewStop::CiFailed, out);
                return;
            }
            let m = &mut w.nodes[i];
            m.run_failures += 1;
            if m.run_failures < 2 {
                set_phase(m, Phase::Publishing); // enqueue again
            } else {
                m.run_failures = 0;
                m.run_incidents = (m.run_incidents + 1).min(2);
                let needs_rob = m.run_incidents >= 2;
                raise_incident(w, i, needs_rob, out);
            }
        }
        Fact::Conflict => {
            let m = &mut w.nodes[i];
            if matches!(m.phase, Phase::Publishing | Phase::Queued) {
                set_phase(m, Phase::Rebasing);
            }
        }
        Fact::UpstreamStalled => {
            let m = &mut w.nodes[i];
            if m.phase != Phase::Queued || m.waiting_incident {
                return;
            }
            m.run_incidents = (m.run_incidents + 1).min(2);
            let needs_rob = m.run_incidents >= 2;
            raise_incident(w, i, needs_rob, out);
        }
        Fact::IncidentCleared { by_rob } => {
            let m = &mut w.nodes[i];
            if m.waiting_incident && (by_rob || !m.incident_needs_rob) {
                m.waiting_incident = false;
                m.incident_needs_rob = false;
            }
        }
    }
}
}

kernel_fn! {
/// Merge fact rows / Q18 / #771: a late merge earns credit once; other merges
/// enter teardown. Even an unapproved late merge emits at most four notices.
pub(super) fn pr_merged(w: &mut World, i: usize, head: HeadId, checks_passed: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned as int <= old(w).nodes@[i as int].credit.earned as int + 1,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent == old(w).nodes@[i as int].credit.spent,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 4,
        final(w).nodes@[i as int].credit.earned as int + merge_capacity(final(w).nodes@[i as int])
            <= old(w).nodes@[i as int].credit.earned as int + 1,

        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let m = &mut w.nodes[i];
    if !m.had_pr || m.merged.is_some() {
        return;
    }
    let approved = m.approved == Some(head) && checks_passed;
    match m.phase {
        Phase::Closed(o) => {
            if !matches!(o, Outcome::Stopped | Outcome::Replaced) {
                return;
            }
            // A late merge corrects the record and earns its credit once.
            m.merged = Some(head);
            m.phase = Phase::Closed(Outcome::Merged);
            merge_credit(m);
            notice(out, i, Notice::Note(NoteKind::LateMerge));
            notice(out, i, Notice::Incident { needs_rob: false });
        }
        _ => {
            m.merged = Some(head);
            m.pr_open = false;
            enter_stopping(m, Outcome::Merged);
        }
    }
    if !approved {
        notice(out, i, Notice::Incident { needs_rob: false });
        notice(out, i, Notice::Note(NoteKind::UnapprovedMerge));
    }
}
}

// ---------------------------------------------------------------- commands

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
pub(super) fn on_command(w: &mut World, i: usize, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == if id > old(w).nodes@[i as int].last_cmd {
            id
        } else { old(w).nodes@[i as int].last_cmd },
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    if id <= w.nodes[i].last_cmd {
        return; // a repeated command
    }
    w.nodes[i].last_cmd = id;
    match command {
        Command::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
        Command::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
        Command::Pause => {
            let m = &mut w.nodes[i];
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                m.paused = true;
            }
        }
        Command::Resume => w.nodes[i].paused = false,
        Command::Answer { question, answer } => on_answer(w, i, question, answer, out),
    }
}
}

kernel_fn! {
/// Question answer rows / #771: cleanup, continuation and funding permissions.
pub(super) fn allowed(kind: QKind, answer: Answer) -> (result: bool) [
    ensures result == match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    },
] {
    match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
pub(super) fn on_answer(w: &mut World, i: usize, id: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let Some(q) = w.nodes[i].question else { return };
    if q.id != id {
        return; // an answer to a question that is no longer open
    }
    match w.nodes[i].phase {
        Phase::Stopping(_) => match answer {
            Answer::Retry => w.nodes[i].question = None,
            Answer::MarkHandled => {
                let m = &mut w.nodes[i];
                m.question = None;
                mark_handled(m);
            }
            _ => {}
        },
        Phase::AwaitingRob => {
            if answer == Answer::Unclear {
                // One clarifying question. After that the question just stays open.
                if !q.clarified {
                    let next = w.next_question;
                    w.next_question += 1;
                    w.nodes[i].question = Some(Question {
                        id: next,
                        clarified: true,
                        ..q
                    });
                    notice(out, i, Notice::Ask { question: next });
                }
                return;
            }
            if !allowed(q.kind, answer) {
                return;
            }
            match answer {
                Answer::Continue => go_on(w, i, q, 0, out),
                Answer::TryAgain => {
                    let m = &mut w.nodes[i];
                    m.attempt = 1;
                    m.review_restarted = false;
                    m.round = 0;
                    m.replanned = false;
                    go_on(
                        w,
                        i,
                        Question {
                            cost: q.cost.max(1),
                            ..q
                        },
                        0,
                        out,
                    );
                }
                Answer::AddCredit(k) => {
                    go_on(w, i, q, k, out);
                }
                Answer::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
                Answer::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
                _ => {}
            }
        }
        _ => {}
    }
}
}

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase.
pub(super) fn go_on(w: &mut World, i: usize, q: Question, deposit: u8,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX, q.cost <= 1,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).nodes@[i as int].credit.start as int
            + old(w).nodes@[i as int].credit.earned as int
            + old(w).nodes@[i as int].credit.added as int
                >= old(w).nodes@[i as int].credit.spent as int,
        old(w).nodes@[i as int].credit.withheld ==> q.cost == 0,
        old(w).nodes@[i as int].credit.added as int + 255 <= u32::MAX as int,
        old(w).nodes@[i as int].credit.spent < u32::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int].question.is_none(),
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let m = &mut w.nodes[i];
    m.credit.fund_and_pay(deposit, q.cost);
    m.question = None;
    match q.cont {
        Cont::Ingesting => set_phase(m, Phase::Ingesting),
        Cont::Auditing => set_phase(m, Phase::Auditing),
        Cont::Unplanned => set_phase(m, Phase::Unplanned),
        Cont::Planning { must_split } => {
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split });
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
        }
        Cont::Ready(r) => set_phase(m, Phase::Ready(r)),
        Cont::Stop(o) => enter_stopping(m, o),
        Cont::Nowhere => enter_stopping(m, Outcome::Stopped),
    }
}
}

kernel_fn! {
/// Cleanup question row / #771: handling cleanup allocates no monotone counter.
pub(super) fn mark_handled(m: &mut Node) -> (result: ()) [
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).phase == old(m).phase,
        final(m).last_cmd == old(m).last_cmd,
] {
    if let Some(Item::Teardown(kind)) = m.current() {
        match kind {
            EffectKind::StopRun => {
                m.run = None;
                m.pool = false;
            }
            EffectKind::WithdrawJob => m.job = None,
            EffectKind::ClosePr => m.pr_open = false,
            EffectKind::RemoveWorktree => m.worktree = false,
            _ => {}
        }
        m.pos += 1;
    }
}
}

