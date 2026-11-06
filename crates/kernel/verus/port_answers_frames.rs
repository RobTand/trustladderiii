//! #806: per-answer consistency and shared Kani assertion frames.
use super::*;
use super::port_answers_model::*;

verus! {
/// State-table continue answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table try again answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table add credit answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table change request answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table stop answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table unclear answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table retry answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// State-table mark handled answer row / #806: exact real-handler frame.
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
    reveal(retirement::node_rules); reveal(answered); reveal(continued);
    reveal(handled); reveal(retirement::stopped);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

}
