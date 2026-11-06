//! #806: exact frames for the real question-answer handlers.
use super::*;

verus! {
/// Q18 / #806: a deposit pays the continuation after topping up its shortfall.
#[verifier::opaque]
pub(super) open spec fn funded(c: Credit, deposit: u8, cost: u8) -> Credit {
    let balance = c.start as int + c.earned as int + c.added as int
        + deposit as int - c.spent as int;
    Credit { added: (c.added as int + deposit as int
        + if cost as int > balance { cost as int - balance } else { 0int }) as u64,
        spent: (c.spent + cost) as u64, ..c }
}

/// AwaitingRob continuation rows / #806: exact result of the real go_on body.
#[verifier::opaque]
pub(super) open spec fn rephased(n: Node, phase: Phase) -> Node { retirement::phased(n, phase) }

/// AwaitingRob continuation rows / #806: exact result of the real go_on body.
#[verifier::opaque]
pub(super) open spec fn continued(n: Node, q: Question, deposit: u8) -> Node {
    let paid = Node { credit: funded(n.credit, deposit, q.cost), question: None, ..n };
    match q.cont {
        Cont::Ingesting => rephased(paid, Phase::Ingesting),
        Cont::Auditing => rephased(paid, Phase::Auditing),
        Cont::Unplanned => rephased(paid, Phase::Unplanned),
        Cont::Planning { must_split } => rephased(Node { plan_retries: 0, ..paid },
            Phase::Planning { must_split }),
        Cont::Ready(r) => rephased(paid, Phase::Ready(r)),
        Cont::Stop(o) => retirement::stopped(paid, o),
        Cont::Nowhere => retirement::stopped(paid, Outcome::Stopped),
    }
}

/// Stopping cleanup answers / #806: only the current teardown resource is cleared.
#[verifier::opaque]
pub(super) open spec fn handled(n: Node) -> Node {
    if n.phase is Stopping {
        Node {
            pos: if n.pos <= 2 || n.pos == 4 { (n.pos + 1) as u8 } else { n.pos },
            run: if n.pos == 0 { None } else { n.run },
            pool: if n.pos == 0 { false } else { n.pool },
            job: if n.pos == 1 { None } else { n.job },
            pr_open: if n.pos == 2 { false } else { n.pr_open },
            worktree: if n.pos == 4 { false } else { n.worktree },
            ..n
        }
    } else { n }
}

/// AwaitingRob offered-answer table / #806, checked against real allowed.
pub(super) open spec fn offered(kind: QKind, answer: Answer) -> bool {
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

/// AwaitingRob / Stopping / #806: complete answer frame, including stale questions.
#[verifier::opaque]
pub(super) open spec fn answered(n: Node, id: u64, answer: Answer, next: u64) -> Node {
    match n.question {
        None => n,
        Some(q) => if q.id != id { n } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => Node { question: None, ..n },
                Answer::MarkHandled => handled(Node { question: None, ..n }),
                _ => n,
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if q.clarified { n } else {
                    Node { question: Some(Question { id: next, clarified: true, ..q }), ..n }
                }
            } else if !offered(q.kind, answer) { n } else { match answer {
                Answer::Continue => continued(n, q, 0),
                Answer::TryAgain => continued(Node { attempt: 1, review_restarted: false,
                    round: 0, replanned: false, ..n }, Question { cost: if q.cost > 1 { q.cost } else { 1 }, ..q }, 0),
                Answer::AddCredit(k) => continued(n, q, k),
                Answer::ChangeRequest => retirement::stopped(n, Outcome::Replaced),
                Answer::Stop => retirement::stopped(n, Outcome::Stopped),
                _ => n,
            } },
            _ => n,
        } },
    }
}

/// Unclear answer row / #806: allocate one fresh clarification, then stutter.
pub(super) open spec fn clarifies(n: Node, id: u64, answer: Answer) -> bool {
    n.phase == Phase::AwaitingRob && answer == Answer::Unclear
        && match n.question { Some(q) => q.id == id && !q.clarified, None => false }
}

/// Command answer row / #806: duplicate command IDs stutter before question matching.
#[verifier::opaque]
pub(super) open spec fn commanded(n: Node, cmd: u64, id: u64, answer: Answer, next: u64) -> Node {
    if cmd <= n.last_cmd { n }
    else { answered(Node { last_cmd: cmd, ..n }, id, answer, next) }
}

/// #806: exact question-counter update, including duplicate commands.
#[verifier::opaque]
pub(super) open spec fn next_question(n: Node, cmd: u64, id: u64, answer: Answer, next: u64) -> u64 {
    if cmd > n.last_cmd && clarifies(n, id, answer) { (next + 1) as u64 } else { next }
}

/// #806 / Q54: every shared check_nodes, check_outputs and answer-specific assertion.
#[verifier::opaque]
pub(super) open spec fn shared(before: Node, after: Node, id: u64) -> bool {
    after.proposed == before.proposed && (before.adopted ==> after.adopted)
        && after.next_seq >= before.next_seq
        && (before.phase is Closed ==> after.phase == before.phase)
        && after.credit.earned == before.credit.earned
        && after.credit.merge_credited == before.credit.merge_credited
        && after.review_stop == before.review_stop
        && after.attempt <= if before.attempt > 1 { before.attempt } else { 1u8 }
        && (before.replanned || !after.replanned)
        && (after.phase is Planning ==> before.phase is Planning || before.phase == Phase::AwaitingRob)
        && (after != Node { last_cmd: after.last_cmd, ..before } ==> {
            (match before.question { Some(q) => q.id == id, None => false })
                && match after.question { Some(q) => q.id != id, None => true }
        })
        && (match after.question {
            Some(q) => q.kind != QKind::CantMeet || before.question == Some(q)
                || match before.question { Some(p) => before.phase == Phase::AwaitingRob
                    && p.kind == q.kind && p.cont == q.cont && p.cost == q.cost
                    && !p.clarified && q.clarified, None => false },
            None => true,
        })
}
}
