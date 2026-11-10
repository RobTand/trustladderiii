//! Reply producers: projections of returned node work and deferred world calls.
use super::*;

verus! {
pub(super) open spec fn prepared(r: ReplyChange) -> Node {
    match r { ReplyChange::Done { node, .. } | ReplyChange::Ask { node, .. }
        | ReplyChange::Incident { node, .. } => node }
}

pub(super) open spec fn finish(nq: u64, r: ReplyChange) -> Node {
    match r {
        ReplyChange::Done { node, .. } => node,
        ReplyChange::Ask { node, kind, cont, cost } => retirement::phased(Node {
            holds: false,
            question: Some(Question { id: nq, kind, cont, cost, clarified: false }),
            ..node
        }, Phase::AwaitingRob),
        ReplyChange::Incident { node, needs_rob } => retirement::incident(node, needs_rob),
    }
}

pub(super) open spec fn next_question(nq: u64, r: ReplyChange) -> u64 {
    match r { ReplyChange::Ask { .. } => (nq + 1) as u64, _ => nq }
}

pub(super) open spec fn item(nq: u64, r: ReplyChange) -> Option<Notice> {
    match r {
        ReplyChange::Done { note, .. } => note,
        ReplyChange::Ask { .. } => Some(Notice::Ask { question: nq }),
        ReplyChange::Incident { needs_rob, .. } => Some(Notice::Incident { needs_rob }),
    }
}

/// Debt's exact Ask/Incident suffix exports are still dependencies. This
/// relation records only their existing output contracts until those land.
pub(super) open spec fn resolved(r: ReplyChange, i: int, ob: Outbox, oa: Outbox) -> bool {
    match r {
        ReplyChange::Done { note: None, .. } => oa == ob,
        ReplyChange::Done { note: Some(note), .. } => oa.len == ob.len + 1
            && oa.items@ == ob.items@.update(ob.len as int,
                Some(Out::Notice { node: NodeId(i as u64), notice: note })),
        ReplyChange::Ask { .. } => oa.len == ob.len + 1,
        ReplyChange::Incident { .. } => oa.len == ob.len + 1
            && (retirement::notices_only(ob) ==> retirement::notices_only(oa)),
    }
}

pub(super) open spec fn callable(r: ReplyChange) -> bool {
    match r {
        ReplyChange::Ask { node, .. } => node.generation < u64::MAX,
        ReplyChange::Incident { node, .. } => holding(node.phase) ==> node.generation < u64::MAX,
        _ => true,
    }
}

pub(super) open spec fn frames(b: Node, n: Node) -> bool {
    n.repo == b.repo && n.last_cmd == b.last_cmd && n.next_seq == b.next_seq
        && n.proposed == b.proposed && n.adopted == b.adopted
        && n.credit.earned == b.credit.earned
        && n.credit.merge_credited == b.credit.merge_credited
}

pub(super) open spec fn retry_bound(b: Node, nq: u64, r: ReplyChange) -> bool {
    let n = finish(nq, r);
    callable(r) && frames(b, n) && n.footprint == b.footprint
        && (settle_shape(b) ==> settle_shape(n))
        && n.generation as int <= b.generation as int + 1
        && n.credit.start == b.credit.start && n.credit.added == b.credit.added
        && n.credit.spent as int <= b.credit.spent as int + 1
}

pub(super) open spec fn reply_bound(b: Node, nq: u64, r: ReplyChange) -> bool {
    let n = finish(nq, r);
    callable(r) && frames(b, n)
        && (settle_shape(b) ==> settle_shape(n))
        && n.generation as int <= b.generation as int + 1
        && n.credit.start as int <= b.credit.start as int + CREDIT_START_STEP_BUDGET
        && n.credit.added == b.credit.added
        && n.credit.spent as int <= b.credit.spent as int + 1
}

pub(super) open spec fn spent(n: Node) -> Node {
    Node { credit: Credit { spent: (n.credit.spent + 1) as u64, ..n.credit }, ..n }
}

pub(super) open spec fn room_fast(n: Node, step: ReviewStop) -> ReplyChange {
    ReplyChange::Done { node: retirement::stopped(
        Node { review_stop: Some(step), ..n }, Outcome::Stopped), note: None }
}

pub(super) open spec fn room_paid(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(Node { plan_retries: 0, ..spent(n) },
        Phase::Planning { must_split: true }), note: None }
}

pub(super) open spec fn room_ask(n: Node) -> ReplyChange {
    ReplyChange::Ask { node: n, kind: QKind::OutOfCredit,
        cont: Cont::Planning { must_split: true }, cost: 1 }
}

#[verifier::opaque]
pub(super) open spec fn room(n: Node, fast: bool, step: ReviewStop) -> ReplyChange {
    if fast { room_fast(n, step) }
    else {
        let m = Node { holds: false, ..n };
        if transition_safety::can_spend_one(n) { room_paid(m) }
        else { room_ask(m) }
    }
}

pub(super) open spec fn failed_prefix(n: Node) -> Node {
    Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
        generation: (n.generation + 1) as u64,
        run_failures: (n.run_failures + 1) as u8, ..retries_facts::abandoned(n) }
}

pub(super) open spec fn failed_incident(n: Node) -> ReplyChange {
    let node = Node { run_failures: 0,
        run_incidents: if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 }, ..n };
    ReplyChange::Incident { node, needs_rob: node.run_incidents >= 2 }
}

pub(super) open spec fn failed_restart(n: Node) -> ReplyChange {
    ReplyChange::Done { node: n, note: None }
}

pub(super) open spec fn failed_change(n: Node) -> ReplyChange {
    let node = failed_prefix(n);
    if node.run_failures >= 2 { failed_incident(node) } else { failed_restart(node) }
}

#[verifier::opaque]
pub(super) open spec fn plan_retry(n: Node, fast: bool, cant_meet: bool) -> ReplyChange {
    let m = Node { plan_retries: if n.plan_retries < 255 { (n.plan_retries + 1) as u8 }
        else { 255u8 }, ..n };
    if fast && cant_meet { ReplyChange::Done { node: retirement::stopped(
        Node { review_stop: Some(ReviewStop::PlanCantMeet), ..m }, Outcome::Stopped), note: None } }
    else if m.plan_retries < 2 { ReplyChange::Done {
        node: retirement::phased(m, m.phase), note: None } }
    else { ReplyChange::Ask { node: Node { plan_retries: 0, ..m },
        kind: QKind::CantMeet,
        cont: Cont::Planning { must_split: matches!(n.phase, Phase::Planning { must_split: true }) },
        cost: 1 } }
}

#[verifier::opaque]
pub(super) open spec fn rung(n: Node, fast: bool, step: ReviewStop) -> ReplyChange {
    if fast { ReplyChange::Done { node: retirement::stopped(
        Node { review_stop: Some(step), ..n }, Outcome::Stopped), note: None } }
    else if n.attempt < 3 {
        let m = Node { attempt: (n.attempt + 1) as u8, review_restarted: false, round: 0, ..n };
        if transition_safety::can_spend_one(n) { ReplyChange::Done {
            node: retirement::phased(spent(m), if m.holds { Phase::Making }
                else { Phase::Ready(Resume::Making) }), note: None } }
        else { ReplyChange::Ask { node: m, kind: QKind::OutOfCredit,
            cont: Cont::Ready(Resume::Making), cost: 1 } }
    } else if !n.replanned {
        let m = Node { replanned: true, holds: false, ..n };
        if transition_safety::can_spend_one(n) { ReplyChange::Done {
            node: retirement::phased(Node { plan_retries: 0, ..spent(m) },
                Phase::Planning { must_split: false }), note: Some(Notice::Note(NoteKind::Replanned)) } }
        else { ReplyChange::Ask { node: m, kind: QKind::OutOfCredit,
            cont: Cont::Planning { must_split: false }, cost: 1 } }
    } else { ReplyChange::Ask { node: n, kind: QKind::CantMeet,
        cont: Cont::Ready(Resume::Making), cost: 1 } }
}

#[verifier::opaque]
pub(super) open spec fn failed_round(n: Node, fast: bool, step: ReviewStop) -> ReplyChange {
    if n.round == 0 { ReplyChange::Done {
        node: retirement::phased(Node { round: 1, ..n }, if n.holds { Phase::Making }
            else { Phase::Ready(Resume::Making) }), note: None } }
    else { rung(n, fast, step) }
}

#[verifier::opaque]
pub(super) open spec fn recheck(n: Node, target: Phase) -> ReplyChange {
    let m = Node { rechecks: if n.rechecks < 255 { (n.rechecks + 1) as u8 }
        else { 255u8 }, ..n };
    let free = m.rechecks <= FREE_RECHECKS;
    if free || transition_safety::can_spend_one(m) {
        let paid = if free { m } else { spent(m) };
        ReplyChange::Done { node: retirement::phased(Node {
            holds: holding(target) && paid.holds, ..paid }, target), note: None }
    } else { ReplyChange::Ask { node: m, kind: QKind::OutOfCredit,
        cont: Cont::Ready(if target == Phase::Reviewing { Resume::Reviewing }
            else { Resume::Checking }), cost: 1 } }
}

pub(super) open spec fn plan(n: Node, fast: bool, reply: Reply) -> ReplyChange {
    match reply {
        Reply::PlanLeaf { footprint, fits } => {
            let must_split = matches!(n.phase, Phase::Planning { must_split: true });
            if fits && !footprint::footprint_empty(footprint) && !must_split {
                ReplyChange::Done { node: retirement::phased(Node {
                    footprint, attempt: 1, review_restarted: false, round: 0, plan_retries: 0, ..n
                }, Phase::Ready(Resume::Making)), note: None }
            } else { plan_retry(n, fast, false) }
        },
        Reply::PlanSplit => ReplyChange::Ask { node: n, kind: QKind::NeedsSplitting,
            cont: Cont::Nowhere, cost: 0 },
        Reply::PlanCantMeet => plan_retry(n, fast, true),
        _ => ReplyChange::Done { node: n, note: None },
    }
}

#[verifier::opaque]
pub(super) open spec fn rebase(n: Node, reply: Reply) -> ReplyChange {
    match reply {
        Reply::RebaseClean { head } => recheck(Node { head: Some(head), checked: None,
            review_restarted: n.head == Some(head) && n.review_restarted, ..n }, Phase::Checking),
        Reply::RebaseNeedsChanges => {
            if transition_safety::can_spend_one(n) { rebase_paid(n) }
            else { rebase_ask(n) }
        },
        _ => ReplyChange::Done { node: n, note: None },
    }
}

#[verifier::opaque]
pub(super) open spec fn work(n: Node, fast: bool, reply: Reply) -> ReplyChange {
    match reply {
        Reply::MakerDone { head } => work_made(n, head),
        Reply::MakerCantMeet => rung(n, fast, ReviewStop::MakerCantMeet),
        Reply::MakerOutOfRoom => room(n, fast, ReviewStop::MakerOutOfRoom),
        Reply::ChecksDone { pass, in_footprint, .. } => {
            if pass && in_footprint { work_checked(n) }
            else { failed_round(n, fast, ReviewStop::ChecksFailed) }
        },
        Reply::Approved { head } => work_approved(n, head),
        Reply::ChangesRequested => failed_round(n, fast, ReviewStop::ChangesRequested),
        Reply::ReviewerOutOfRoom => {
            if n.review_restarted { room(n, fast, ReviewStop::ReviewerOutOfRoom) }
            else { recheck(Node { review_restarted: true, ..n }, Phase::Reviewing) }
        },
        _ => rebase(n, reply),
    }
}

pub(super) open spec fn ingested(n: Node, size: u8, early_end: Option<EarlyEnd>) -> Node {
    retirement::phased(Node {
        credit: Credit { ready: true,
            start: if n.credit.ready { n.credit.start } else { (size as u64 + RESERVE) as u64 },
            ..n.credit }, early_end, ..n
    }, Phase::Auditing)
}

pub(super) open spec fn ingest(n: Node, reply: Reply) -> ReplyChange {
    match reply {
        Reply::IngestDone { size } => ReplyChange::Done { node: ingested(n, size, None), note: None },
        Reply::IngestEarlyEnd(e) => ReplyChange::Done { node: ingested(n, 1, Some(e)), note: None },
        Reply::AuditAgrees => match n.early_end {
            Some(e) => ReplyChange::Done { node: retirement::stopped(n, match e {
                EarlyEnd::Duplicate => Outcome::Duplicate,
                EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
                EarlyEnd::NotActionable => Outcome::NotActionable,
            }), note: None },
            None => ReplyChange::Done { node: retirement::phased(n, Phase::Unplanned),
                note: Some(Notice::MeasuresPosted) },
        },
        Reply::AuditDisagrees => ReplyChange::Done { node: port_t860_contracts::audit_node(n), note: None },
        Reply::ProtectedPaths => ReplyChange::Done { node: retirement::stopped(n, Outcome::NotActionable), note: None },
        _ => ReplyChange::Done { node: n, note: None },
    }
}

pub(super) open spec fn plan_check(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::stopped(
        Node { review_stop: Some(ReviewStop::PlanCantMeet), ..n }, Outcome::Stopped), note: None }
}

pub(super) open spec fn files_paid(n: Node, union: Footprint) -> Node {
    retirement::phased(Node { footprint: union, holds: false, ..spent(n) },
        Phase::Ready(Resume::Making))
}

pub(super) open spec fn files_ask(n: Node, union: Footprint) -> Node {
    Node { footprint: union, ..n }
}

#[verifier::opaque]
pub(super) open spec fn files(n: Node, fast: bool, files: Footprint, fits: bool,
    union: Footprint) -> ReplyChange {
    if fits && !footprint::footprint_empty(files) && transition_safety::can_spend_one(n) {
        ReplyChange::Done { node: files_paid(n, union), note: None }
    } else if fits && !footprint::footprint_empty(files) && fast {
        ReplyChange::Ask { node: files_ask(n, union), kind: QKind::OutOfCredit,
            cont: Cont::Ready(Resume::Making), cost: 1 }
    } else { room(n, fast, ReviewStop::MakerNeedsFiles) }
}

#[verifier::opaque]
pub(super) open spec fn rung_paid(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(spent(n), if n.holds { Phase::Making } else { Phase::Ready(Resume::Making) }), note: None }
}
#[verifier::opaque]
pub(super) open spec fn rung_ask(n: Node) -> ReplyChange {
    ReplyChange::Ask { node: n, kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1 }
}
#[verifier::opaque]
pub(super) open spec fn rung_replan_paid(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(Node { plan_retries: 0, ..spent(n) }, Phase::Planning { must_split: false }), note: Some(Notice::Note(NoteKind::Replanned)) }
}
#[verifier::opaque]
pub(super) open spec fn rung_replan_ask(n: Node) -> ReplyChange {
    ReplyChange::Ask { node: n, kind: QKind::OutOfCredit, cont: Cont::Planning { must_split: false }, cost: 1 }
}
#[verifier::opaque]
pub(super) open spec fn rung_cant_meet(n: Node) -> ReplyChange {
    ReplyChange::Ask { node: n, kind: QKind::CantMeet, cont: Cont::Ready(Resume::Making), cost: 1 }
}
#[verifier::opaque]
pub(super) open spec fn round_free(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(Node { round: 1, ..n }, if n.holds { Phase::Making } else { Phase::Ready(Resume::Making) }), note: None }
}
#[verifier::opaque]
pub(super) open spec fn plan_again(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(n, n.phase), note: None }
}
#[verifier::opaque]
pub(super) open spec fn plan_ask(n: Node) -> ReplyChange {
    ReplyChange::Ask { node: Node { plan_retries: 0, ..n }, kind: QKind::CantMeet, cont: Cont::Planning { must_split: matches!(n.phase, Phase::Planning { must_split: true }) }, cost: 1 }
}
#[verifier::opaque]
pub(super) open spec fn recheck_free(n: Node, target: Phase) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(held(n, target), target), note: None }
}
#[verifier::opaque]
pub(super) open spec fn recheck_paid(n: Node, target: Phase) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(held(spent(n), target), target), note: None }
}
#[verifier::opaque]
pub(super) open spec fn recheck_ask(n: Node, target: Phase) -> ReplyChange {
    ReplyChange::Ask { node: n, kind: QKind::OutOfCredit, cont: Cont::Ready(if target == Phase::Reviewing { Resume::Reviewing } else { Resume::Checking }), cost: 1 }
}

#[verifier::opaque]
pub(super) open spec fn bumped(n: Node) -> Node {
    Node { attempt: (n.attempt + 1) as u8, review_restarted: false, round: 0, ..n }
}
#[verifier::opaque]
pub(super) open spec fn replanning(n: Node) -> Node {
    Node { replanned: true, holds: false, ..n }
}
#[verifier::opaque]
pub(super) open spec fn rechecked(n: Node) -> Node {
    Node { rechecks: if n.rechecks < 255 { (n.rechecks + 1) as u8 } else { 255u8 }, ..n }
}
#[verifier::opaque]
pub(super) open spec fn plan_counted(n: Node) -> Node {
    Node { plan_retries: if n.plan_retries < 255 { (n.plan_retries + 1) as u8 } else { 255u8 }, ..n }
}
#[verifier::opaque]
pub(super) open spec fn unheld(n: Node) -> Node { Node { holds: false, ..n } }
#[verifier::opaque]
pub(super) open spec fn held(n: Node, target: Phase) -> Node {
    if holding(target) { n } else { Node { holds: false, ..n } }
}
pub(super) open spec fn work_made(n: Node, head: HeadId) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(Node { head: Some(head), checked: None, ..n }, Phase::Checking), note: None }
}
pub(super) open spec fn work_checked(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(Node { checked: n.head, ..n }, Phase::Reviewing), note: None }
}
pub(super) open spec fn work_approved(n: Node, head: HeadId) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(Node { approved: Some(head), ..n }, Phase::Publishing), note: None }
}
pub(super) open spec fn rebase_paid(n: Node) -> ReplyChange {
    ReplyChange::Done { node: retirement::phased(spent(n), Phase::Making), note: None }
}
pub(super) open spec fn rebase_ask(n: Node) -> ReplyChange {
    ReplyChange::Ask { node: n, kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1 }
}

}
