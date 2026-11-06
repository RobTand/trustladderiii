//! #806: exact results for acknowledgement and owner-command ports.
use super::*;

verus! {
/// State-table owner commands: Answers belong to the separate answer port.
pub(super) open spec fn owner_command(c: Command) -> bool {
    matches!(c, Command::Stop | Command::Pause | Command::Resume | Command::ChangeRequest)
}

/// State-table command rows: duplicate IDs stutter, including their output.
#[verifier::opaque]
pub(super) open spec fn command_node(n: Node, id: u64, c: Command) -> Node {
    if id <= n.last_cmd { n } else {
        let fresh = Node { last_cmd: id, ..n };
        match c {
            Command::Stop => retirement::stopped(fresh, Outcome::Stopped),
            Command::ChangeRequest => retirement::stopped(fresh, Outcome::Replaced),
            Command::Pause => if n.phase is Stopping || n.phase is Closed { fresh }
                else { Node { paused: true, ..fresh } },
            Command::Resume => Node { paused: false, ..fresh },
            _ => fresh,
        }
    }
}

/// #806: keep the owner-command equation opaque while proving the pre-existing progress contracts.
#[verifier::opaque]
pub(super) open spec fn command_result(b: Node, a: Node, id: u64, c: Command) -> bool {
    owner_command(c) ==> a == command_node(b, id, c)
}

/// State-table Pause: direct assignment frame, separate from stopping and Resume.
#[verifier::opaque]
pub(super) open spec fn paused_node(n: Node, id: u64) -> Node {
    if id <= n.last_cmd { n } else { Node { last_cmd: id,
        paused: n.paused || !(n.phase is Stopping || n.phase is Closed), ..n } }
}

/// #806: keep the exact Pause assignment opaque in every other command arm.
#[verifier::opaque]
pub(super) open spec fn pause_post(b: Node, a: Node, id: u64, c: Command) -> bool {
    c == Command::Pause ==> a == paused_node(b, id)
}

/// #806: the command-ID assignment is composed before any handler is called.
#[verifier::opaque]
pub(super) open spec fn id_post(b: Node, a: Node, id: u64) -> bool {
    a == (Node { last_cmd: id, ..b })
}

/// #806: owner commands preserve the complete output and question counter.
#[verifier::opaque]
pub(super) open spec fn command_io(qb: u64, qa: u64, ob: Outbox, oa: Outbox, c: Command) -> bool {
    owner_command(c) ==> qa == qb && oa == ob
}

/// #806: opaque resource-cleanup equation for real Confirmed composition.
#[verifier::opaque]
pub(super) open spec fn cleanup_post(b: Node, a: Node, k: EffectKind) -> bool {
    a == confirmed_cleanup(b, k)
}

/// #806: opaque agenda equation for real Confirmed composition.
#[verifier::opaque]
pub(super) open spec fn agenda_post(b: Node, a: Node, k: EffectKind) -> bool {
    a == confirmed_agenda_node(b, k)
}

/// #806: isolate the exact Confirmed result from its progress postconditions.
#[verifier::opaque]
pub(super) open spec fn confirmed_post(b: Node, a: Node, k: EffectKind) -> bool {
    a == confirmed_node(b, k)
}

/// #806: keep the exact Permanent equation opaque in the executable handler.
#[verifier::opaque]
pub(super) open spec fn permanent_post(b: Node, a: Node, q: u64, k: EffectKind) -> bool {
    a == permanent_node(b, q, k)
}

/// #806: the second-failure counter assignments precede any operation abandonment.
#[verifier::opaque]
pub(super) open spec fn counted_post(b: Node, a: Node) -> bool {
    a == (Node { fx_failures: 0,
        fx_incidents: if b.fx_incidents < 2 { (b.fx_incidents + 1) as u8 } else { 2u8 }, ..b })
}

/// #806: opaque real Retryable result, exported without unfolding the node equation.
#[verifier::opaque]
pub(super) open spec fn retry_post(b: Node, a: Node, k: EffectKind) -> bool {
    a == retry_node(b, k)
}

/// #806: isolate the exact failed-delivery result from its arithmetic postconditions.
#[verifier::opaque]
pub(super) open spec fn delivery_post(b: Node, a: Node, k: EffectKind) -> bool {
    a == failed_delivery(b, k)
}

/// #806: the allocation/storage frame common to these single-node transitions.
#[verifier::opaque]
pub(super) open spec fn world_frame(b: World, a: World, i: usize) -> bool {
    a.nodes@.len() == b.nodes@.len()
        && (forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j])
        && a.next_ticket == b.next_ticket && a.gates@ == b.gates@
        && a.file_sets@ == b.file_sets@ && a.dependencies@ == b.dependencies@
        && a.pool_capacity == b.pool_capacity && a.fail_fast == b.fail_fast
}

/// State-table owner command frame, including the complete unchanged outbox.
#[verifier::opaque]
pub(super) open spec fn command_post(b: World, a: World, i: usize, id: u64,
    c: Command, ob: Outbox, oa: Outbox) -> bool {
    world_frame(b, a, i) && a.next_question == b.next_question && oa == ob
        && a.nodes@[i as int] == command_node(b.nodes@[i as int], id, c)
}

/// #806: canonical empty output used by the executable apply theorem.
pub(super) open spec fn empty() -> Outbox {
    Outbox { items: [None; OUTBOX], len: 0 }
}

/// #806: every check_nodes assertion shared by the eight retired harnesses.
#[verifier::opaque]
pub(super) open spec fn node_transition(b: Node, a: Node) -> bool {
    a.proposed == b.proposed && (b.adopted ==> a.adopted) && a.next_seq >= b.next_seq
        && (b.phase is Closed ==> a.phase == b.phase)
        && a.credit.earned as int <= b.credit.earned as int + 1
        && (a.credit.earned > b.credit.earned ==> !b.credit.merge_credited && a.credit.merge_credited)
}

/// Q54 / #806: command and Ack common assertions, including the publication exception.
#[verifier::opaque]
pub(super) open spec fn q54(b: Node, a: Node, fail_fast: bool, publication: bool) -> bool {
    (publication ==> a.review_stop == Some(ReviewStop::PublicationBody))
        && (!fail_fast ==> a.review_stop == b.review_stop || publication)
        && (fail_fast ==> a.attempt <= if b.attempt > 1 { b.attempt } else { 1u8 })
        && (fail_fast ==> b.replanned || !a.replanned)
        && (fail_fast && a.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && (fail_fast ==> match a.question {
            Some(q) => q.kind != QKind::CantMeet || b.question == Some(q), None => true,
        })
        && (a.review_stop != b.review_stop ==> b.review_stop.is_none()
            && a.phase == Phase::Stopping(Outcome::Stopped) && a.credit == b.credit && a.question.is_none())
}
}

verus! {
/// State-table confirmed delivery: reset delivery failures and release only its target.
#[verifier::opaque]
pub(super) open spec fn confirmed_cleanup(n: Node, k: EffectKind) -> Node {
    Node { fx_failures: 0, fx_incidents: 0,
        run: if k == EffectKind::StopRun { None } else { n.run },
        pool: n.pool && k != EffectKind::StopRun,
        job: if k == EffectKind::WithdrawJob { None } else { n.job },
        pr_open: n.pr_open && k != EffectKind::ClosePr,
        worktree: n.worktree && k != EffectKind::RemoveWorktree, ..n }
}

/// State-table confirmed agenda: an adopted worktree enters Checking; starts await replies.
#[verifier::opaque]
pub(super) open spec fn confirmed_agenda_node(n: Node, k: EffectKind) -> Node {
    if !matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
        && (current_item(n) == Some(Item::Do(k)) || current_item(n) == Some(Item::Teardown(k))) {
        if k == EffectKind::PrepareWorktree && !n.adopted && n.proposed.is_some() {
            retirement::phased(Node { adopted: true, head: n.proposed, checked: None, ..n }, Phase::Checking)
        } else { Node { pos: (n.pos + 1) as u8, ..n } }
    } else { n }
}

/// State-table confirmed acknowledgement: compose resource cleanup and agenda consumption.
#[verifier::opaque]
pub(super) open spec fn confirmed_node(n: Node, k: EffectKind) -> Node {
    confirmed_agenda_node(confirmed_cleanup(n, k), k)
}

/// State-table failed delivery: operation abandonment happens only on every second failure.
#[verifier::opaque]
pub(super) open spec fn failed_delivery(n: Node, k: EffectKind) -> Node {
    if n.fx_failures == 0 { Node { fx_failures: 1, ..n } } else {
        let incidents = if n.fx_incidents < 2 { (n.fx_incidents + 1) as u8 } else { 2u8 };
        let counted = Node { fx_failures: 0, fx_incidents: incidents, ..n };
        if matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            Node { run: None, job: None, pool: false,
                effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
                    || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None },
                pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
                generation: (n.generation + 1) as u64, ..counted }
        } else { counted }
    }
}

/// State-table retry emission: allocate a fresh effect key and conservatively record its target.
#[verifier::opaque]
pub(super) open spec fn emitted_node(n: Node, k: EffectKind) -> Node {
    Node { next_seq: (n.next_seq + 1) as u64,
        effect: Some(InFlight { seq: n.next_seq, kind: k, cancelling: false }),
        worktree: n.worktree || k == EffectKind::PrepareWorktree,
        pr_open: n.pr_open || k == EffectKind::OpenPr,
        had_pr: n.had_pr || k == EffectKind::OpenPr, ..n }
}

/// State-table retry rows: first retry emits; the second raises the saturating incident.
#[verifier::opaque]
pub(super) open spec fn retry_node(n: Node, k: EffectKind) -> Node {
    let failed = failed_delivery(n, k);
    if n.fx_failures == 0 { emitted_node(failed, k) }
    else { retirement::incident(failed, failed.fx_incidents >= 2) }
}

/// State-table permanent effect failure: exact continuation through Ready for holding phases.
pub(super) open spec fn failed_cont(n: Node) -> Cont {
    match n.phase {
        Phase::Ingesting => Cont::Ingesting, Phase::Auditing => Cont::Auditing,
        Phase::Planning { must_split } => Cont::Planning { must_split },
        Phase::Making => Cont::Ready(Resume::Making),
        Phase::Checking => Cont::Ready(Resume::Checking),
        Phase::Reviewing => Cont::Ready(Resume::Reviewing),
        Phase::Publishing | Phase::Queued => Cont::Ready(Resume::Publishing),
        Phase::Rebasing => Cont::Ready(Resume::Rebasing),
        _ => Cont::Unplanned,
    }
}

/// State-table permanent effect failure: cleanup stays put; other phases ask the owner.
#[verifier::opaque]
pub(super) open spec fn permanent_node(n: Node, q: u64, k: EffectKind) -> Node {
    if n.phase is Stopping {
        Node { question: Some(Question { id: q, kind: QKind::CleanupFailed,
            cont: Cont::Nowhere, cost: 0, clarified: false }), ..n }
    } else {
        let abandoned = if matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            Node { run: None, job: None, pool: false,
                effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
                    || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n }
        } else { n };
        retirement::phased(Node { holds: false, question: Some(Question { id: q,
            kind: QKind::EffectFailed, cont: failed_cont(n), cost: 0, clarified: false }), ..abandoned }, Phase::AwaitingRob)
    }
}

/// #629 / Q54: only a fresh noncancelling OpenPr body refusal stops for review.
pub(super) open spec fn publication(n: Node, seq: u64, ack: Ack) -> bool {
    ack == Ack::BodyPolicyRefused && match n.effect {
        Some(e) => e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr,
        None => false,
    }
}

/// State-table Ack rows / #806: exact node result, including stale and cancelled deliveries.
#[verifier::opaque]
pub(super) open spec fn ack_node(n: Node, q: u64, seq: u64, ack: Ack) -> Node {
    match n.effect {
        Some(e) => if e.seq != seq { n } else {
            let cleared = Node { effect: None, ..n };
            if e.cancelling { cleared } else { match ack {
                Ack::Confirmed => confirmed_node(cleared, e.kind),
                Ack::Retryable => retry_node(cleared, e.kind),
                Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr =>
                    retirement::stopped(Node { review_stop: Some(ReviewStop::PublicationBody), ..cleared }, Outcome::Stopped),
                _ => permanent_node(cleared, q, e.kind),
            } }
        },
        None => n,
    }
}

/// State-table Ack rows: permanent failures allocate exactly one question ID.
pub(super) open spec fn asks(n: Node, seq: u64, ack: Ack) -> bool {
    match n.effect {
        Some(e) => e.seq == seq && !e.cancelling
            && (ack == Ack::Permanent || (ack == Ack::BodyPolicyRefused && e.kind != EffectKind::OpenPr)),
        None => false,
    }
}

/// #806: exact output append, including every unused slot in the Kani equality assertion.
#[verifier::opaque]
pub(super) open spec fn appended(ob: Outbox, oa: Outbox, emitted: Seq<Out>) -> bool {
    oa.len as int == ob.len as int + emitted.len()
        && forall|j: int| 0 <= j < OUTBOX ==> #[trigger] oa.items@[j] ==
            if ob.len <= j < ob.len + emitted.len() { Some(emitted[j - ob.len]) }
            else { ob.items@[j] }
}

/// State-table retry output: fresh effect or the every-second-failure incident.
pub(super) open spec fn retry_emissions(n: Node, i: usize, k: EffectKind) -> Seq<Out> {
    if n.fx_failures == 0 {
        seq![Out::Effect { node: NodeId(i as u64), seq: n.next_seq, kind: k,
            head: if matches!(k, EffectKind::Push | EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::Enqueue) {
                n.head
            } else { None } }]
    } else { seq![Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: n.fx_incidents >= 1 } }] }
}

/// State-table permanent failure output: incident followed by its exact owner question.
pub(super) open spec fn permanent_emissions(q: u64, i: usize) -> Seq<Out> {
    seq![Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: true } },
        Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: q } }]
}

/// State-table delivery outputs: no emissions for stale/cancelled/confirmed/body-refused OpenPr.
#[verifier::opaque]
pub(super) open spec fn ack_emissions(n: Node, q: u64, i: usize, seq: u64, ack: Ack) -> Seq<Out> {
    match n.effect {
        Some(e) => if e.seq != seq || e.cancelling || ack == Ack::Confirmed || publication(n, seq, ack) { seq![] }
            else if ack == Ack::Retryable { retry_emissions(n, i, e.kind) }
            else { permanent_emissions(q, i) },
        None => seq![],
    }
}

/// #806: Retryable emits an exact fresh effect on the first failure; the incident branch emits only notices.
#[verifier::opaque]
pub(super) open spec fn retry_output(n: Node, i: usize, k: EffectKind, ob: Outbox, oa: Outbox) -> bool {
    if n.fx_failures == 0 { appended(ob, oa, retry_emissions(n, i, k)) }
    else { oa.len as int == ob.len as int + 1
        && (retirement::notices_only(ob) ==> retirement::notices_only(oa)) }
}

/// #806: complete output safety; exact slot equality is retained on every refusal/Permanent path.
#[verifier::opaque]
pub(super) open spec fn ack_output(n: Node, q: u64, i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox) -> bool {
    match n.effect {
        Some(e) => if e.seq == seq && !e.cancelling && ack == Ack::Retryable {
            retry_output(n, i, e.kind, ob, oa)
        } else { appended(ob, oa, ack_emissions(n, q, i, seq, ack)) },
        None => appended(ob, oa, ack_emissions(n, q, i, seq, ack)),
    }
}

/// #806: local Ack result, composed with storage frames after the handler match.
#[verifier::opaque]
pub(super) open spec fn ack_local(b: Node, a: Node, qb: u64, qa: u64,
    i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox) -> bool {
    a == ack_node(b, qb, seq, ack)
        && qa as int == qb as int + (if asks(b, seq, ack) { 1int } else { 0int })
        && ack_output(b, qb, i, seq, ack, ob, oa)
}

/// State-table Ack frame: immutable world fields, exact node/question update and complete outbox.
#[verifier::opaque]
pub(super) open spec fn ack_post(b: World, a: World, i: usize, seq: u64, ack: Ack, ob: Outbox, oa: Outbox) -> bool {
    world_frame(b, a, i) && a.nodes@[i as int] == ack_node(b.nodes@[i as int], b.next_question, seq, ack)
        && a.next_question as int == b.next_question as int + (if asks(b.nodes@[i as int], seq, ack) { 1int } else { 0int })
        && ack_output(b.nodes@[i as int], b.next_question, i, seq, ack, ob, oa)
}
}

verus! {
/// State-table Permanent / #806: isolate conditional operation abandonment from the phase match.
#[verifier::opaque]
pub(super) open spec fn permanent_abandon_post(b: Node, a: Node, k: EffectKind) -> bool {
    a == if matches!(k, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
        Node { run: None, job: None, pool: false,
            effect: match b.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
                || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..b }
    } else { b }
}

/// State-table Permanent / #806: literal exact ask_rob result, kept opaque in the caller.
#[verifier::opaque]
pub(super) open spec fn asked_post(b: Node, a: Node, q: u64, cont: Cont) -> bool {
    a == (Node { holds: false, question: Some(Question { id: q,
        kind: QKind::EffectFailed, cont, cost: 0, clarified: false }),
        phase: Phase::AwaitingRob, generation: (b.generation + 1) as u64,
        pos: 0, ticket: None,
        effect: match b.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }, ..b })
}
}

verus! {

/// State-table MarkHandled / #806: isolate the single position assignment.
#[verifier::opaque]
pub(super) open spec fn handled_increment(b: Node, a: Node) -> bool {
    a == (Node { pos: (b.pos + 1) as u8, ..b })
}
}

verus! {
/// State-table confirmed adoption / #806: three assignments before the real set_phase call.
#[verifier::opaque]
pub(super) open spec fn adoption_post(b: Node, a: Node) -> bool {
    a == (Node { adopted: true, head: b.proposed, checked: None, ..b })
}
}

verus! {
/// State-table Ack / #806: isolate the actual delivery-key clearing assignment.
#[verifier::opaque]
pub(super) open spec fn cleared_post(b: Node, a: Node) -> bool {
    a == (Node { effect: None, ..b })
}
}
