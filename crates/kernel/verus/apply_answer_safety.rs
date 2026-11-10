//! Derived properties over the exact Apply answer result.
//! Design 3 audit check_design3.py sha256 0a29b5cd568e41b6c5177631b6f5a488ff7038a601ae6312f6bd9c162f789d67
//! Design 3 audit k5_exec_compare.py sha256 407c83dc0a2df8cf3f102d3e6ee9d81b687adc481a15029c8d2ad770337120d1
//! Design 3 audit k5_post_check.py sha256 d538b7df6aeacfb8c0584dc3e7e0178d4d24c8835af1f9628624e3c10b10f43a
//! Arm split audit build_arm_split.py sha256 bf6096a6eb17bb350ebce13f5e09893c651bcec9009e951b58a22ecc931fa637
use super::*;
use super::apply_answer_values::*;
verus! {

pub(super) proof fn stopped_frame(n: Node, outcome: Outcome)
    requires !(n.phase is Closed || n.phase is Stopping)
        ==> n.generation < u64::MAX,
    ensures retirement::stopped(n, outcome).repo == n.repo,
        retirement::stopped(n, outcome).footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(retirement::stopped(n, outcome))),
        retirement::stopped(n, outcome).generation as int == n.generation as int
        + if n.phase is Closed || n.phase is Stopping { 0int } else { 1int },
        retirement::stopped(n, outcome).phase == match n.phase {
        Phase::Closed(o) => Phase::Closed(o),
        Phase::Stopping(o) => Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }),
        _ => Phase::Stopping(outcome),
        },
        retirement::stopped(n, outcome).next_seq == n.next_seq,
        retirement::stopped(n, outcome).credit == n.credit,
        retirement::stopped(n, outcome).last_cmd == n.last_cmd,
        retirement::stopped(n, outcome).review_stop == n.review_stop,
        (n.phase is Closed || n.phase is Stopping) ==> retirement::stopped(n, outcome) == (Node {
        phase: retirement::stopped(n, outcome).phase, ..n
        }),
        !(n.phase is Closed || n.phase is Stopping) ==>
        retirement::stopped(n, outcome).pos == 0 && retirement::stopped(n, outcome).ticket.is_none() && retirement::stopped(n, outcome).question.is_none()
        && !retirement::stopped(n, outcome).waiting_incident && !retirement::stopped(n, outcome).incident_needs_rob,
        retirement::stopped(n, outcome).proposed == n.proposed,
        retirement::stopped(n, outcome).adopted == n.adopted,
        retirement::stopped(n, outcome).attempt == n.attempt,
        retirement::stopped(n, outcome).replanned == n.replanned,
        retirement::stopped(n, outcome).credit.merge_credited == n.credit.merge_credited,
        retirement::stopped(n, outcome).credit.withheld == n.credit.withheld,
{
    reveal(retirement::stopped);
}

pub(super) proof fn stopped_progress(n: Node, outcome: Outcome)
    requires !(n.phase is Closed || n.phase is Stopping)
        ==> n.generation < u64::MAX,
    ensures (!n.paused && !n.waiting_incident && n.effect.is_none()
        && !(n.phase is Closed || n.phase is Stopping)
        ==> distance(retirement::stopped(n, outcome)) < distance(n)),
{
    reveal(retirement::stopped);
    reveal(distance);
    reveal_with_fuel(phase_rank, 3);
}

pub(super) proof fn continued_frame(n: Node, q: Question, deposit: u8)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures handler_safety::continued_node(n, q, deposit).repo == n.repo,
        handler_safety::continued_node(n, q, deposit).footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(handler_safety::continued_node(n, q, deposit))),
        handler_safety::continued_node(n, q, deposit).generation as int <= n.generation as int + 1,
        handler_safety::continued_node(n, q, deposit).next_seq == n.next_seq,
        handler_safety::continued_node(n, q, deposit).credit.start == n.credit.start,
        handler_safety::continued_node(n, q, deposit).credit.earned == n.credit.earned,
        handler_safety::continued_node(n, q, deposit).credit.added as int <= n.credit.added as int + 255,
        handler_safety::continued_node(n, q, deposit).credit.spent as int <= n.credit.spent as int + 1,
        handler_safety::continued_node(n, q, deposit).last_cmd == n.last_cmd,
        handler_safety::continued_node(n, q, deposit).question.is_none(),
        handler_safety::continued_node(n, q, deposit).proposed == n.proposed,
        handler_safety::continued_node(n, q, deposit).adopted == n.adopted,
        handler_safety::continued_node(n, q, deposit).review_stop == n.review_stop,
        handler_safety::continued_node(n, q, deposit).attempt == n.attempt,
        handler_safety::continued_node(n, q, deposit).replanned == n.replanned,
        handler_safety::continued_node(n, q, deposit).credit.merge_credited == n.credit.merge_credited,
        handler_safety::continued_node(n, q, deposit).credit.withheld == n.credit.withheld,
        handler_safety::continued_node(n, q, deposit).credit.withheld == n.credit.withheld,
{
    reveal(handler_safety::continued_node);
    match q.cont {
        Cont::Stop(o) => stopped_frame(Node { credit: handler_safety::paid_credit(n.credit, deposit, q.cost), question: None, ..n }, o),
        Cont::Nowhere => stopped_frame(Node { credit: handler_safety::paid_credit(n.credit, deposit, q.cost), question: None, ..n }, Outcome::Stopped),
        _ => {},
    }
}

pub(super) proof fn continued_resources(n: Node, q: Question, deposit: u8)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures handler_safety::continued_node(n, q, deposit).credit.withheld == n.credit.withheld,
        (handler_safety::continued_node(n, q, deposit).holds ==> n.holds),
        (handler_safety::continued_node(n, q, deposit).pool ==> n.pool),
        handler_safety::continued_node(n, q, deposit).ticket == n.ticket || handler_safety::continued_node(n, q, deposit).ticket.is_none(),
{
    reveal(handler_safety::continued_node);
    match q.cont {
        Cont::Stop(o) => handler_safety::stopped_no_grants(Node { credit: handler_safety::paid_credit(n.credit, deposit, q.cost), question: None, ..n }, o),
        Cont::Nowhere => handler_safety::stopped_no_grants(Node { credit: handler_safety::paid_credit(n.credit, deposit, q.cost), question: None, ..n }, Outcome::Stopped),
        _ => {},
    }
}

pub(super) proof fn continued_progress(n: Node, q: Question, deposit: u8)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures (n.phase == Phase::AwaitingRob && n.question == Some(q) && !n.paused
            && !n.waiting_incident && n.effect.is_none()
            ==> distance(handler_safety::continued_node(n, q, deposit)) < distance(n)),
{
    reveal(handler_safety::continued_node);
    reveal(distance);
    reveal_with_fuel(phase_rank, 3);
    match q.cont {
        Cont::Stop(o) => stopped_progress(Node { credit: handler_safety::paid_credit(n.credit, deposit, q.cost), question: None, ..n }, o),
        Cont::Nowhere => stopped_progress(Node { credit: handler_safety::paid_credit(n.credit, deposit, q.cost), question: None, ..n }, Outcome::Stopped),
        _ => {},
    }
}

pub(super) proof fn continued_step(n: Node, q: Question, deposit: u8)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures n.phase == Phase::AwaitingRob ==> handler_safety::command_step(n, handler_safety::continued_node(n, q, deposit)),
{
    continued_frame(n, q, deposit);
    if n.phase == Phase::AwaitingRob { handler_safety::continue_step(n, handler_safety::continued_node(n, q, deposit)); }
}

pub(super) proof fn retry_frame(n: Node, q: Question)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures ({
            let b = try_again_node(n);
            let q1 = Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q };
            true
            && retried_answer(n, q).0.repo == b.repo && retried_answer(n, q).0.footprint == b.footprint
            && (settle_shape(b) ==> settle_shape(retried_answer(n, q).0))
            && retried_answer(n, q).0.generation as int <= b.generation as int + 1
            && retried_answer(n, q).0.next_seq == b.next_seq
            && retried_answer(n, q).0.credit.start == b.credit.start && retried_answer(n, q).0.credit.earned == b.credit.earned
            && retried_answer(n, q).0.credit.added as int <= b.credit.added as int + 255
            && retried_answer(n, q).0.credit.spent as int <= b.credit.spent as int + 1
            && retried_answer(n, q).0.last_cmd == b.last_cmd
            && retried_answer(n, q).0.question.is_none()
            && retried_answer(n, q).0.proposed == b.proposed && retried_answer(n, q).0.adopted == b.adopted
            && retried_answer(n, q).0.review_stop == b.review_stop && retried_answer(n, q).0.attempt == 1
            && !retried_answer(n, q).0.replanned
        }),
        retried_answer(n, q).1.is_none(),
{
    reveal(retried_answer);
    reveal(continued_answer);
    continued_frame(answers::try_again_node(n), Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q }, 0);
}

pub(super) proof fn retry_resources(n: Node, q: Question)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures (retried_answer(n, q).0.holds ==> n.holds),
        (retried_answer(n, q).0.pool ==> n.pool),
        retried_answer(n, q).0.ticket == n.ticket || retried_answer(n, q).0.ticket.is_none(),
        retried_answer(n, q).0.credit.withheld == n.credit.withheld,
{
    reveal(retried_answer);
    reveal(continued_answer);
    continued_resources(answers::try_again_node(n), Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q }, 0);
}

pub(super) proof fn retry_progress(n: Node, q: Question)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures ({ let b = answers::try_again_node(n); let q1 = Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q }; b.phase == Phase::AwaitingRob && b.question == Some(q1) && !b.paused && !b.waiting_incident && b.effect.is_none() ==> distance(retried_answer(n, q).0) < distance(b) }),
{
    reveal(retried_answer);
    reveal(continued_answer);
    continued_progress(answers::try_again_node(n), Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q }, 0);
}

pub(super) proof fn retry_step(n: Node, q: Question)
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures n.phase == Phase::AwaitingRob ==> handler_safety::command_step(n, retried_answer(n, q).0),
{
    reveal(retried_answer);
    reveal(continued_answer);
    continued_frame(answers::try_again_node(n), Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q }, 0);
    if n.phase == Phase::AwaitingRob { handler_safety::continue_step(n, retried_answer(n, q).0); }
}

pub(super) proof fn answer_progress(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (handler_safety::node_ok(n, nq)
            && answer_progress_input(n, id, answer)
            ==> distance(answered(n, nq, id, answer).0) < distance(n)),
{
    reveal(answered);
    if !(handler_safety::node_ok(n, nq) && answer_progress_input(n, id, answer)) { return; }
    reveal(distance);
    match n.question {
        None => {  },
        Some(q) => if q.id != id {  } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => {  },
                Answer::MarkHandled => { cleanup_lemmas::teardown_progress(Node { question: None, ..n }, handler_safety::handled_node(Node { question: None, ..n })); },
                _ => {  },
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if !q.clarified {  } else {  }
            } else if !permits(q.kind, answer) {  } else { match answer {
                Answer::Continue => { reveal(continued_answer);
continued_progress(n, q, 0); },
                Answer::TryAgain => { retry_progress(n, q); },
                Answer::AddCredit(k) => { reveal(continued_answer);
continued_progress(n, q, k); },
                Answer::ChangeRequest => { stopped_progress(n, Outcome::Replaced); },
                Answer::Stop => { stopped_progress(n, Outcome::Stopped); },
                _ => {  },
            }},
            _ => {  },
        }},
    }
}

pub(super) proof fn answer_frame(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (n.phase is Closed ==> answered(n, nq, id, answer).0.phase is Closed),
        answered(n, nq, id, answer).0.repo == n.repo,
        answered(n, nq, id, answer).0.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(answered(n, nq, id, answer).0)),
        answered(n, nq, id, answer).0.generation as int <= n.generation as int + 1,
        answered(n, nq, id, answer).0.next_seq == n.next_seq,
        answered(n, nq, id, answer).0.credit.start == n.credit.start,
        answered(n, nq, id, answer).0.credit.earned == n.credit.earned,
        answered(n, nq, id, answer).0.credit.added as int <= n.credit.added as int + 255,
        answered(n, nq, id, answer).0.credit.spent as int <= n.credit.spent as int + 1,
        answered(n, nq, id, answer).0.last_cmd == n.last_cmd,
        answered(n, nq, id, answer).1 as int <= nq as int + 1,
        answered(n, nq, id, answer).0.proposed == n.proposed,
        answered(n, nq, id, answer).0.adopted == n.adopted,
        (n.phase is Closed ==> answered(n, nq, id, answer).0.phase == n.phase),
{
    reveal(answered);
    match n.question {
        None => {  },
        Some(q) => if q.id != id {  } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => {  },
                Answer::MarkHandled => {  },
                _ => {  },
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if !q.clarified {  } else {  }
            } else if !permits(q.kind, answer) {  } else { match answer {
                Answer::Continue => { reveal(continued_answer);
continued_frame(n, q, 0); },
                Answer::TryAgain => { retry_frame(n, q); },
                Answer::AddCredit(k) => { reveal(continued_answer);
continued_frame(n, q, k); },
                Answer::ChangeRequest => { stopped_frame(n, Outcome::Replaced); },
                Answer::Stop => { stopped_frame(n, Outcome::Stopped); },
                _ => {  },
            }},
            _ => {  },
        }},
    }
}

pub(super) proof fn answer_step(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures handler_safety::command_step(n, answered(n, nq, id, answer).0),
{
    reveal(answered);
    match n.question {
        None => { handler_safety::same_step(n); },
        Some(q) => if q.id != id { handler_safety::same_step(n); } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => { handler_safety::field_step(n, Node { question: None, ..n }); },
                Answer::MarkHandled => { handler_safety::handled_cleanup_step(n); },
                _ => { handler_safety::same_step(n); },
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if !q.clarified { handler_safety::clarify_step(n, q, nq); } else { handler_safety::same_step(n); }
            } else if !permits(q.kind, answer) { handler_safety::same_step(n); } else { match answer {
                Answer::Continue => { reveal(continued_answer);
continued_step(n, q, 0); },
                Answer::TryAgain => { retry_step(n, q); },
                Answer::AddCredit(k) => { reveal(continued_answer);
continued_step(n, q, k); },
                Answer::ChangeRequest => { handler_safety::stop_step(n, Outcome::Replaced); },
                Answer::Stop => { handler_safety::stop_step(n, Outcome::Stopped); },
                _ => { handler_safety::same_step(n); },
            }},
            _ => { handler_safety::same_step(n); },
        }},
    }
}

pub(super) proof fn answer_consumed(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (handler_safety::node_ok(n, nq) && answered(n, nq, id, answer).0 != n ==> (match n.question {
            Some(q) => q.id == id,
            None => false,
        }) && (match answered(n, nq, id, answer).0.question {
            Some(q) => q.id != id,
            None => true,
        })),
{
    reveal(answered);
    match n.question {
        None => {  },
        Some(q) => if q.id != id {  } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => {  },
                Answer::MarkHandled => {  },
                _ => {  },
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if !q.clarified {  } else {  }
            } else if !permits(q.kind, answer) {  } else { match answer {
                Answer::Continue => { reveal(continued_answer);
continued_frame(n, q, 0); },
                Answer::TryAgain => { retry_frame(n, q); },
                Answer::AddCredit(k) => { reveal(continued_answer);
continued_frame(n, q, k); },
                Answer::ChangeRequest => { stopped_frame(n, Outcome::Replaced); },
                Answer::Stop => { stopped_frame(n, Outcome::Stopped); },
                _ => {  },
            }},
            _ => {  },
        }},
    }
}

pub(super) proof fn answer_resources(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (answered(n, nq, id, answer).0.holds ==> n.holds),
        (answered(n, nq, id, answer).0.pool ==> n.pool),
        answered(n, nq, id, answer).0.ticket == n.ticket || answered(n, nq, id, answer).0.ticket.is_none(),
        answered(n, nq, id, answer).0.credit.withheld == n.credit.withheld,
{
    reveal(answered);
    match n.question {
        None => {  },
        Some(q) => if q.id != id {  } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => {  },
                Answer::MarkHandled => {  },
                _ => {  },
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if !q.clarified {  } else {  }
            } else if !permits(q.kind, answer) {  } else { match answer {
                Answer::Continue => { reveal(continued_answer);
continued_resources(n, q, 0); },
                Answer::TryAgain => { retry_resources(n, q); },
                Answer::AddCredit(k) => { reveal(continued_answer);
continued_resources(n, q, k); },
                Answer::ChangeRequest => { handler_safety::stopped_no_grants(n, Outcome::Replaced); },
                Answer::Stop => { handler_safety::stopped_no_grants(n, Outcome::Stopped); },
                _ => {  },
            }},
            _ => {  },
        }},
    }
}

pub(super) proof fn answer_question_order(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures nq <= answered(n, nq, id, answer).1,
{
    reveal(answered);
}

pub(super) proof fn answer_rules(n: Node, nq: u64, id: u64, answer: Answer)
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (handler_safety::node_rules_v(n, nq) ==> handler_safety::node_rules_v(answered(n, nq, id, answer).0, answered(n, nq, id, answer).1)),
{
    if !handler_safety::node_rules_v(n, nq) { return; }
    reveal(answered);
    match n.question {
        None => {},
        Some(q) => if q.id != id {} else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => { handler_safety::clear_cleanup_rules_v(n, nq); },
                Answer::MarkHandled => { handler_safety::cleanup_handled_rules_v(n, nq); },
                _ => {},
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if !q.clarified { handler_safety::clarified_rules_v(n, nq, q); }
            } else if !permits(q.kind, answer) {} else { match answer {
                Answer::Continue => { continued_answer_rules(n, q, 0, nq); },
                Answer::TryAgain => { retried_answer_rules(n, q, nq); },
                Answer::AddCredit(k) => { continued_answer_rules(n, q, k, nq); },
                Answer::ChangeRequest => { handler_safety::stopped_rules_v(n, Outcome::Replaced, nq); },
                Answer::Stop => { handler_safety::stopped_rules_v(n, Outcome::Stopped, nq); },
                _ => {},
            }},
            _ => {},
        }},
    }
}

pub(super) proof fn continued_answer_rules(n: Node, q: Question, deposit: u8, nq: u64)
    requires handler_safety::node_rules_v(n, nq), n.phase == Phase::AwaitingRob,
        n.question == Some(q), deposit == 0 || q.kind == QKind::OutOfCredit,
        q.cost <= 1, n.generation < u64::MAX,
        n.credit.added as int + 255 <= u64::MAX as int, n.credit.spent < u64::MAX,
    ensures handler_safety::node_rules_v(continued_answer(n, q, deposit).0, nq),
{
    reveal(continued_answer);
    handler_safety::continued_rules_v(n, q, deposit, nq);
}

pub(super) proof fn retried_answer_rules(n: Node, q: Question, nq: u64)
    requires handler_safety::node_rules_v(n, nq), n.phase == Phase::AwaitingRob,
        n.question == Some(q), q.kind == QKind::CantMeet, n.generation < u64::MAX,
        n.credit.added as int + 255 <= u64::MAX as int, n.credit.spent < u64::MAX,
    ensures handler_safety::node_rules_v(retried_answer(n, q).0, nq),
{
    handler_safety::retry_rules_v(n, q, nq);
    let q1 = Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q };
    assert(q1 == q);
    handler_safety::continued_rules_v(answers::try_again_node(n), q1, 0, nq);
    reveal(retried_answer);
    reveal(continued_answer);
}

pub(super) proof fn continued_arms_bridge(n: Node, q: Question, deposit: u8)
    ensures handler_safety::continued_node(n, q, deposit) == continued_arms(n, q, deposit),
{
    reveal(handler_safety::continued_node);
}

pub(super) proof fn awaiting_arms_bridge(n: Node, nq: u64, id: u64, answer: Answer, q: Question)
    requires n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures answered(n, nq, id, answer) == awaiting_arms(n, nq, answer, q),
{
    reveal(answered);
}


}
