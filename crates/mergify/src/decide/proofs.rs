//! Kani proofs of the Mergify queue decisions (#735).
//!
//! Each proof states its property over every value of the typed inputs. Check
//! and event lists hold at most `N` (3) entries: enough for two members of one
//! group plus an outsider, which is every shape the grouping rules tell apart
//! (a newer, an older or an equal ID in the same group, or another group).
//! Longer lists only repeat those comparisons, which the concrete scenarios
//! still exercise. Every loop runs at most `N` times, so unwind `N + 1` (4)
//! is exact, and Kani's unwinding assertions check it.
//!
//! Every proof has `kani::cover!` checks, and `tools/kani.py` fails a proof
//! whose covers are not all satisfied, so no proof passes vacuously.

use super::check::*;
use super::facts::*;
use super::observe::*;
use super::translate::*;

const N: usize = 3;

macro_rules! proof {
    ($name:ident, $bound:literal, $body:expr) => {
        #[kani::proof]
        #[kani::unwind($bound)]
        fn $name() {
            $body;
        }
    };
}

/// K-q52: Q52 and the other setup evidence never become a kernel fact, so no
/// failure is counted and no credit charged. A missing check opens its
/// incident only while an enqueue is pending on the approved head; a report
/// clears it. A required context is missing only when nothing reported it and
/// its absence is knowable; it is final once every suite on the head
/// completed.
fn check_q52() {
    let reported: bool = kani::any();
    let expects_app: bool = kani::any();
    let statuses: bool = kani::any();
    let missing = required_missing(reported, expects_app, statuses);
    assert!(!(reported && missing), "a reported context is missing");
    assert!(
        missing == (!reported && (expects_app || statuses)),
        "missing misjudged"
    );
    let suites: [Status; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let mut all = len > 0;
    let mut i = 0;
    while i < len {
        if suites[i] != Status::Completed {
            all = false;
        }
        i += 1;
    }
    let completed = suites_completed(&suites[..len]);
    assert!(completed == all, "suites misjudged");

    let node: NodeView = kani::any();
    let head_current: bool = kani::any();
    let approved: bool = kani::any();
    let enqueuing = node.phase == Phase::Publishing && node.enqueue_pending;
    let routes = [
        route(Kind::ChecksUnavailable, node, head_current, approved),
        route(Kind::CheckMissing, node, head_current, approved),
        route(Kind::QueueBlocked, node, head_current, approved),
        route(Kind::CheckReported, node, head_current, approved),
    ];
    // Four fixed checks rather than a loop, so unwind 4 still bounds the proof.
    assert!(
        !matches!(routes[0], Route::Kernel(_)),
        "setup evidence reached the kernel"
    );
    assert!(
        !matches!(routes[1], Route::Kernel(_)),
        "Q52 evidence reached the kernel"
    );
    assert!(
        !matches!(routes[2], Route::Kernel(_)),
        "#447 evidence reached the kernel"
    );
    assert!(
        !matches!(routes[3], Route::Kernel(_)),
        "a Q52 report reached the kernel"
    );
    assert!(
        routes[0] == Route::SetupIncident,
        "unreadable checks misrouted"
    );
    assert!(
        routes[1]
            == if enqueuing && approved {
                Route::MissingIncident
            } else {
                Route::Skip
            },
        "missing check misrouted"
    );
    assert!(
        routes[2]
            == if (node.phase == Phase::Queued || enqueuing) && approved {
                Route::BlockedIncident
            } else {
                Route::Skip
            },
        "blocked queue misrouted"
    );
    assert!(routes[3] == Route::ClearIncident, "a report does not clear");
    kani::cover!(
        routes[1] == Route::MissingIncident,
        "a missing check opens its incident"
    );
    kani::cover!(
        enqueuing && !approved && routes[1] == Route::Skip,
        "a missing check on another head is skipped"
    );
    kani::cover!(
        node.phase == Phase::Queued && routes[1] == Route::Skip && approved,
        "a queued node raises no missing check"
    );
    kani::cover!(
        node.phase == Phase::Queued && routes[2] == Route::BlockedIncident,
        "a queued node can be blocked"
    );
    kani::cover!(
        missing && !statuses && expects_app,
        "an expected app's check is missing without statuses"
    );
    kani::cover!(
        !missing && !reported && !expects_app && !statuses,
        "an unreadable status proves nothing"
    );
    kani::cover!(completed && len == N, "all suites completed");
    kani::cover!(!completed && len == 0, "no suites proves nothing");
}

/// K-translate-closed: a closed node takes only a merge it has not recorded,
/// and that merge reaches the kernel as a merge.
fn check_translate_closed() {
    let kind: Kind = kani::any();
    let node: NodeView = kani::any();
    kani::assume(node.phase == Phase::Closed);
    let admitted = admit(kind, node);
    assert!(
        admitted == (matches!(kind, Kind::Merged { .. }) && !node.merged),
        "a closed node took the wrong fact"
    );
    if admitted {
        assert!(
            matches!(
                route(kind, node, kani::any(), kani::any()),
                Route::Kernel(KernelFact::PrMerged { .. })
            ),
            "a late merge did not reach the kernel"
        );
    }
    kani::cover!(admitted, "a late merge corrects a closed node");
    kani::cover!(
        !admitted && matches!(kind, Kind::Merged { .. }),
        "a recorded merge is not taken twice"
    );
    kani::cover!(
        !admitted && matches!(kind, Kind::CiFailed { .. }),
        "a failure after close is dropped"
    );
}

/// K-head-ci-consume: a head-check failure reaches only a queued node or one
/// still enqueuing; an open node takes every other fact.
fn check_translate_head_ci() {
    let kind: Kind = kani::any();
    let node: NodeView = kani::any();
    kani::assume(node.phase != Phase::Closed);
    let admitted = admit(kind, node);
    match kind {
        Kind::CiFailed {
            head_check: true, ..
        } => assert!(
            admitted
                == (node.phase == Phase::Queued
                    || node.phase == Phase::Publishing && node.enqueue_pending),
            "head-check failure misadmitted"
        ),
        _ => assert!(admitted, "an open node refused a fact"),
    }
    let head_ci = matches!(
        kind,
        Kind::CiFailed {
            head_check: true,
            ..
        }
    );
    kani::cover!(
        head_ci && node.phase == Phase::Queued && admitted,
        "queued takes it"
    );
    kani::cover!(
        head_ci && node.phase == Phase::Publishing && admitted,
        "an enqueuing node takes it"
    );
    kani::cover!(
        head_ci && node.phase == Phase::Publishing && !admitted,
        "a publishing node without an enqueue waits"
    );
    kani::cover!(
        head_ci && node.phase == Phase::Other && !admitted,
        "other phases wait"
    );
}

/// K-translate-route: evidence about a queue attempt on a head that is no
/// longer the node's is only recorded; every other kernel-bound fact maps to
/// exactly its kernel fact, with its payload unchanged.
fn check_translate_route() {
    let kind: Kind = kani::any();
    let node: NodeView = kani::any();
    let head_current: bool = kani::any();
    let approved: bool = kani::any();
    let routed = route(kind, node, head_current, approved);
    // The oracle names the attempt kinds itself, independent of
    // `Kind::names_attempt_head`, so a mutant there cannot move both.
    let attempt = matches!(
        kind,
        Kind::CiFailed { .. } | Kind::Conflict | Kind::DequeuedByCommand
    );
    let stale = attempt && !head_current;
    assert!(
        (routed == Route::Record) == stale,
        "only a stale attempt is just recorded"
    );
    if !stale {
        let expected = match kind {
            Kind::Merged { checks_passed } => Some(KernelFact::PrMerged { checks_passed }),
            Kind::CiFailed { infra, .. } => Some(KernelFact::CiFailed { infra }),
            Kind::Conflict => Some(KernelFact::Conflict),
            Kind::DequeuedByPerson | Kind::DequeuedByCommand => {
                Some(KernelFact::PrDequeuedByPerson)
            }
            Kind::ClosedByPerson => Some(KernelFact::PrClosedByPerson),
            Kind::PushByOther => Some(KernelFact::PushByOther),
            _ => None,
        };
        match expected {
            Some(fact) => assert!(routed == Route::Kernel(fact), "wrong kernel fact"),
            None => {
                assert!(
                    matches!(
                        kind,
                        Kind::ChecksUnavailable
                            | Kind::CheckMissing
                            | Kind::QueueBlocked
                            | Kind::CheckReported
                    ),
                    "a non-setup fact missed the kernel"
                );
                assert!(
                    !matches!(routed, Route::Kernel(_) | Route::Record),
                    "setup evidence reached the kernel or was only recorded"
                );
            }
        }
    }
    kani::cover!(stale, "a stale attempt is recorded");
    kani::cover!(
        kind == Kind::Conflict && !head_current,
        "a stale conflict is recorded"
    );
    kani::cover!(
        kind == Kind::DequeuedByCommand && !head_current,
        "a stale dequeue command is recorded"
    );
    kani::cover!(
        routed == Route::Kernel(KernelFact::CiFailed { infra: true }),
        "an infrastructure failure"
    );
    kani::cover!(
        kind == Kind::DequeuedByCommand && routed == Route::Kernel(KernelFact::PrDequeuedByPerson),
        "a dequeue command is a person's dequeue"
    );
}

/// K-ci-classify: a repeated test is never infrastructure; a failure outside
/// the change is, unless repeated; otherwise the upstream classification
/// stands. The decision names which rule applied.
fn check_ci_classify() {
    let upstream: bool = kani::any();
    let outside: bool = kani::any();
    let repeated: bool = kani::any();
    let (infra, decision) = classify_ci(upstream, outside, repeated);
    assert!(
        !(repeated && infra),
        "a repeated test counted as infrastructure"
    );
    if outside && !repeated {
        assert!(infra, "an outside failure is not infrastructure");
    }
    if !outside && !repeated {
        assert!(infra == upstream, "the upstream classification changed");
    }
    assert!(
        decision
            == if repeated {
                Decision::RepeatedTest
            } else if outside {
                Decision::OutsideChange
            } else {
                Decision::UpstreamClassification
            },
        "decision misnamed"
    );
    kani::cover!(repeated && outside && upstream, "a repeat overrides both");
    kani::cover!(outside && !upstream && infra, "outside overrides upstream");
    kani::cover!(
        !outside && !repeated && infra,
        "upstream infrastructure stands"
    );
}

/// K-person-facts: a merged PR yields no person facts; otherwise each event
/// where a person removed the queue label or closed the PR yields its fact,
/// in event order, and nothing else does.
fn check_person_facts() {
    let events: [IssueEvent; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let events = &events[..len];
    let merged: bool = kani::any();
    let mut out: [Option<PersonFact>; N] = [None; N];
    let mut n = 0;
    for fact in person_facts(events, merged) {
        assert!(n < N, "more facts than events");
        out[n] = Some(fact);
        n += 1;
    }
    let mut merge_seen = merged;
    let mut i = 0;
    while i < len {
        if events[i].kind == EventKind::Merged {
            merge_seen = true;
        }
        i += 1;
    }
    if merge_seen {
        assert!(n == 0, "a merged PR yielded a person fact");
    } else {
        let mut k = 0;
        let mut i = 0;
        while i < len {
            let event = events[i];
            let expected = match (event.actor, event.kind) {
                (Actor::Person, EventKind::Unlabeled { queue_label: true }) => {
                    Some(PersonFact::Dequeued { event: event.id })
                }
                (Actor::Person, EventKind::Closed) => Some(PersonFact::Closed { event: event.id }),
                _ => None,
            };
            if let Some(expected) = expected {
                assert!(
                    k < n && out[k] == Some(expected),
                    "a person fact is missing"
                );
                k += 1;
            }
            i += 1;
        }
        assert!(k == n, "an extra person fact");
    }
    kani::cover!(
        n == 2
            && matches!(out[0], Some(PersonFact::Dequeued { .. }))
            && matches!(out[1], Some(PersonFact::Closed { .. })),
        "a dequeue then a close"
    );
    kani::cover!(
        n == 0
            && !merge_seen
            && len == 1
            && events[0].actor == Actor::Bot
            && events[0].kind == EventKind::Closed,
        "a bot's close is ignored"
    );
    kani::cover!(
        n == 0
            && !merged
            && len == 2
            && events[1].kind == EventKind::Merged
            && events[0].actor == Actor::Person
            && events[0].kind == EventKind::Closed,
        "a later merge suppresses a person's close"
    );
}

/// K-facts-outcome: what a queue read yields. A merge-queue PR yields nothing,
/// a merged PR never counts as another's push, and head checks are read only
/// for an open PR whose queue state can still wait or fail. Merge evidence
/// waits for Mergify's final result on the approved head. Each queue state
/// maps to one outcome; the queue report's infrastructure flag is read only
/// when the head checks did not decide.
fn check_facts_outcome() {
    let merge_queue_pr: bool = kani::any();
    let state: QueueState = kani::any();
    let expected_head: bool = kani::any();
    let open: bool = kani::any();
    let read = entry(merge_queue_pr, state, expected_head, open);
    assert!(
        (read == Entry::Ignore) == merge_queue_pr,
        "merge-queue PR misjudged"
    );
    if read == Entry::PushByOther {
        assert!(
            state != QueueState::Merged && !expected_head,
            "a push by another without cause"
        );
    }
    if !merge_queue_pr && (state == QueueState::Merged || expected_head) {
        let head_checks = open
            && matches!(
                state,
                QueueState::Waiting
                    | QueueState::Unknown
                    | QueueState::FailedCi
                    | QueueState::Dequeued
            );
        assert!(read == Entry::Read { head_checks }, "read misjudged");
    }

    let check: Option<QueueCheck> = kani::any();
    let merge_commit: bool = kani::any();
    let plan = merged_plan(expected_head, check, merge_commit);
    let unfinished = check.is_some_and(|check| !check.completed || !check.concluded);
    assert!(
        (plan == MergedPlan::Wait) == (expected_head && unfinished),
        "merge evidence waited wrongly"
    );
    if plan != MergedPlan::Wait {
        assert!(
            plan == if check.is_some_and(|check| check.merged_title) {
                MergedPlan::ViaQueue
            } else if merge_commit {
                MergedPlan::MergeCommit
            } else {
                MergedPlan::NotPassed
            },
            "merge evidence from the wrong source"
        );
    }

    // The check's payload is opaque to the decision, so a byte stands for it.
    let queue: Option<u8> = kani::any();
    let present = queue.is_some();
    let head_infra: Option<bool> = kani::any();
    let report_infra: bool = kani::any();
    let mut read_report = None;
    let outcome = queue_outcome(state, queue, head_infra, |check| {
        read_report = Some(check);
        report_infra
    });
    let expected = match (state, queue) {
        (QueueState::Merged, _) => Some(Outcome::Merged),
        (QueueState::FailedCi, Some(check)) => Some(Outcome::CiFailed {
            check,
            infra: head_infra.unwrap_or(report_infra),
        }),
        (QueueState::Dequeued, Some(check)) => Some(Outcome::CiFailed {
            check,
            infra: head_infra.unwrap_or(true),
        }),
        (QueueState::DequeuedByPerson, Some(check)) => Some(Outcome::DequeuedByCommand(check)),
        (QueueState::Conflict, Some(check)) => Some(Outcome::Conflict(check)),
        _ => None,
    };
    assert!(outcome == expected, "outcome misjudged");
    assert!(
        read_report.is_some() == (state == QueueState::FailedCi && present && head_infra.is_none()),
        "the queue report was read needlessly"
    );
    assert!(
        read_report.is_none() || read_report == queue,
        "another check's report was read"
    );
    kani::cover!(read == Entry::PushByOther, "a push by another");
    kani::cover!(
        read == Entry::Read { head_checks: true },
        "head checks are read"
    );
    kani::cover!(
        state == QueueState::Merged && !expected_head && read == Entry::Read { head_checks: false },
        "a merge on another head is still read"
    );
    kani::cover!(plan == MergedPlan::Wait, "wait for Mergify's result");
    kani::cover!(
        plan == MergedPlan::ViaQueue && !expected_head && unfinished,
        "another head does not wait"
    );
    kani::cover!(
        matches!(outcome, Some(Outcome::CiFailed { infra: false, .. }))
            && state == QueueState::Dequeued,
        "head checks call a dequeue a code failure"
    );
    kani::cover!(read_report.is_some(), "the queue report decides");
}

proof!(q52, 4, check_q52());
proof!(translate_closed, 4, check_translate_closed());
proof!(translate_head_ci, 4, check_translate_head_ci());
proof!(translate_route, 4, check_translate_route());
proof!(ci_classify, 4, check_ci_classify());
proof!(person_facts_total, 4, check_person_facts());
proof!(facts_outcome, 4, check_facts_outcome());
