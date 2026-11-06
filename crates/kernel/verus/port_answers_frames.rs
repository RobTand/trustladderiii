//! #806: per-answer consistency and shared Kani assertion frames.
use super::port_answers_model::*;
use super::*;

verus! {
/// State-table change request answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn change_request_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::ChangeRequest, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::ChangeRequest, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::ChangeRequest, before.next_question), id),
        !commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::ChangeRequest, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::ChangeRequest;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::change_request_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            match answer {
                                Answer::ChangeRequest | Answer::Stop => {
                                    let outcome = if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced };
                                    port_answers_safe::stopped_safe(after, updated, outcome);
                                },
                                _ => {},
                            }
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}

/// State-table stop answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn stop_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::Stop, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::Stop, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::Stop, before.next_question), id),
        !commanded(n, cmd, id, Answer::Stop, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::Stop, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::Stop, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::Stop, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Stop, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::Stop, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Stop, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::Stop;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::stop_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            match answer {
                                Answer::ChangeRequest | Answer::Stop => {
                                    let outcome = if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced };
                                    port_answers_safe::stopped_safe(after, updated, outcome);
                                },
                                _ => {},
                            }
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}

/// State-table unclear answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn unclear_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::Unclear, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::Unclear, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::Unclear, before.next_question), id),
        !commanded(n, cmd, id, Answer::Unclear, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::Unclear, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::Unclear, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::Unclear, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Unclear, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::Unclear, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Unclear, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::Unclear;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::unclear_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            match answer {
                                Answer::ChangeRequest | Answer::Stop => {
                                    let outcome = if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced };
                                    port_answers_safe::stopped_safe(after, updated, outcome);
                                },
                                _ => {},
                            }
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}

/// State-table retry answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn retry_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::Retry, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::Retry, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::Retry, before.next_question), id),
        !commanded(n, cmd, id, Answer::Retry, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::Retry, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::Retry, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::Retry, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Retry, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::Retry, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Retry, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::Retry;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::retry_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            match answer {
                                Answer::ChangeRequest | Answer::Stop => {
                                    let outcome = if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced };
                                    port_answers_safe::stopped_safe(after, updated, outcome);
                                },
                                _ => {},
                            }
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}

/// State-table retry answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn mark_handled_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::MarkHandled, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::MarkHandled, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::MarkHandled, before.next_question), id),
        !commanded(n, cmd, id, Answer::MarkHandled, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::MarkHandled, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::MarkHandled, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::MarkHandled, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::MarkHandled, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::MarkHandled, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::MarkHandled, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::MarkHandled;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::mark_handled_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            match answer {
                                Answer::ChangeRequest | Answer::Stop => {
                                    let outcome = if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced };
                                    port_answers_safe::stopped_safe(after, updated, outcome);
                                },
                                _ => {},
                            }
                        }
                    },
                    Phase::Stopping(_) => {
                        port_answers_safe::handled_safe(after, updated);
                    },
                    _ => {},
                }
            }
        }
    }
}


/// State-table continue answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn continue_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::Continue, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::Continue, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::Continue, before.next_question), id),
        !commanded(n, cmd, id, Answer::Continue, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::Continue, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::Continue, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::Continue, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::Continue, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::Continue, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::Continue, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::Continue;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::continue_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            assert(!updated.credit.ready ==>  q.cost == 0) by { reveal(retirement::node_rules); }
                            port_answers_safe::continued_safe(after, updated, q, 0);
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}



/// State-table try_again answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn try_again_frame(before: World, after: World, n: Node, cmd: u64, id: u64)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::TryAgain, before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::TryAgain, before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::TryAgain, before.next_question), id),
        !commanded(n, cmd, id, Answer::TryAgain, before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::TryAgain, before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::TryAgain, before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::TryAgain, before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::TryAgain, before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::TryAgain, before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::TryAgain, before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::TryAgain;
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::try_again_laws(n, cmd, id, before.next_question);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            let restarted = Node { attempt: 1, review_restarted: false, round: 0, replanned: false, ..updated };
                            port_answers_safe::restarted_safe(after, updated);
                            assert(q.cost == 1 && updated.credit.ready) by { reveal(retirement::node_rules); }
                            port_answers_safe::continued_safe(after, restarted, q, 0);
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}



/// State-table add_credit answer row / #806: exact real-handler frame.
#[verifier::spinoff_prover]
pub(super) proof fn add_credit_frame(before: World, after: World, n: Node, cmd: u64, id: u64, k: u8)
    requires retirement::node_rules(before, n), node_headroom(n),
        before.next_question < u64::MAX,
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(n, cmd, id, Answer::AddCredit(k), before.next_question),
    ensures retirement::node_rules(after, commanded(n, cmd, id, Answer::AddCredit(k), before.next_question)),
        shared(n, commanded(n, cmd, id, Answer::AddCredit(k), before.next_question), id),
        !commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).holds || n.holds,
        !commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).pool || n.pool,
        commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).ticket.is_none()
            || commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).ticket == n.ticket,
        commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).repo == n.repo,
        commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).footprint == n.footprint,
        commanded(n, cmd, id, Answer::AddCredit(k), before.next_question).credit.withheld == n.credit.withheld,
{
    reveal(commanded); reveal(next_question);
    let answer = Answer::AddCredit(k);
    assert(match n.question { Some(q) => q.id < before.next_question, None => true }) by {
        reveal(retirement::node_rules);
    }
    port_answers_laws::add_credit_laws(n, cmd, id, before.next_question, k);
    port_answers_apply::unchanged_node(before, after, n);
    let updated = Node { last_cmd: cmd, ..n };
    assert(retirement::node_rules(after, updated)) by { reveal(retirement::node_rules); }
    reveal(answered);
    if cmd > n.last_cmd {
        if let Some(q) = n.question {
            if q.id == id {
                match n.phase {
                    Phase::AwaitingRob => {
                        if answer == Answer::Unclear {
                            reveal(retirement::node_rules);
                        } else if offered(q.kind, answer) {
                            assert(!updated.credit.ready ==> k == 0 && q.cost == 0) by { reveal(retirement::node_rules); }
                            port_answers_safe::continued_safe(after, updated, q, k);
                        }
                    },
                    Phase::Stopping(_) => {
                        reveal(retirement::node_rules);
                    },
                    _ => {},
                }
            }
        }
    }
}



}
