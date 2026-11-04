//! Kani proofs of the Mergify core's decisions (#735).
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

use crate::check::*;
use crate::facts::*;
use crate::http::*;
use crate::observe::*;
use crate::translate::*;

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

/// Up to `N` arbitrary checks.
fn any_checks() -> ([CheckView; N], usize) {
    let checks: [CheckView; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    (checks, len)
}

fn mergify_check(check: &CheckView, name: Name) -> bool {
    check.head == Head::Current && check.name == name && check.app == App::Mergify
}

/// `i` holds the largest ID among Mergify's `name` checks on the head, and no
/// later one ties it.
fn newest_last(checks: &[CheckView], i: usize, name: Name) -> bool {
    if !mergify_check(&checks[i], name) {
        return false;
    }
    let mut j = 0;
    while j < checks.len() {
        if mergify_check(&checks[j], name)
            && (checks[j].id > checks[i].id || j > i && checks[j].id == checks[i].id)
        {
            return false;
        }
        j += 1;
    }
    true
}

/// The adapter's former grouping, step by step: a `BTreeMap` filled in order
/// with `entry(key).or_insert(check)`, where a strictly larger ID replaces the
/// entry. Returns the kept indices and their count.
fn btreemap_newest(
    checks: &[CheckView],
    same: fn(&CheckView, &CheckView) -> bool,
) -> ([usize; N], usize) {
    let mut kept = [0; N];
    let mut n = 0;
    let mut i = 0;
    while i < checks.len() {
        if checks[i].head == Head::Current {
            let mut found = false;
            let mut k = 0;
            while k < n {
                if same(&checks[kept[k]], &checks[i]) {
                    found = true;
                    if checks[i].id > checks[kept[k]].id {
                        kept[k] = i;
                    }
                }
                k += 1;
            }
            if !found {
                kept[n] = i;
                n += 1;
            }
        }
        i += 1;
    }
    (kept, n)
}

fn kept(kept: &[usize; N], n: usize, i: usize) -> bool {
    let mut k = 0;
    while k < n {
        if kept[k] == i {
            return true;
        }
        k += 1;
    }
    false
}

fn same_app_and_name(a: &CheckView, b: &CheckView) -> bool {
    a.app_key == b.app_key && a.name_key == b.name_key
}

fn same_name(a: &CheckView, b: &CheckView) -> bool {
    a.name_key == b.name_key
}

/// K-observe-select: the selected queue check is Mergify's, on the current
/// head, with the queried name and the largest ID, the last on a tie; there
/// is none only when no check qualifies.
fn check_observe_select() {
    let (checks, len) = any_checks();
    let checks = &checks[..len];
    let name: Name = kani::any();
    let chosen = select(checks, name);
    let mut qualifying = 0;
    let mut j = 0;
    while j < len {
        if mergify_check(&checks[j], name) {
            qualifying += 1;
        }
        j += 1;
    }
    match chosen {
        None => assert!(qualifying == 0, "a qualifying check was not selected"),
        Some(i) => {
            assert!(i < len, "selected index out of range");
            assert!(
                newest_last(checks, i, name),
                "selected check is not the newest"
            );
        }
    }
    kani::cover!(len == N && chosen.is_none(), "no qualifying check");
    kani::cover!(
        len == N
            && chosen == Some(1)
            && mergify_check(&checks[0], name)
            && checks[0].id == checks[1].id,
        "a tie selects the later check"
    );
    kani::cover!(
        len == N
            && chosen == Some(0)
            && checks[1].head == Head::Other
            && checks[1].id > checks[0].id,
        "a newer check on another head is ignored"
    );
    kani::cover!(
        len == N
            && chosen == Some(0)
            && checks[2].app == App::MergifySlug
            && checks[2].head == Head::Current
            && checks[2].name == name
            && checks[2].id > checks[0].id,
        "a newer check from an impostor app is ignored"
    );
}

/// K-observe-merged: the state is Merged exactly when GitHub records a merge;
/// a queue report of a merge that GitHub has not recorded is Unknown.
fn check_observe_merged() {
    let pull: PullView = kani::any();
    let reported: Option<QueueState> = kani::any();
    let mut read = false;
    let state = queue_state(pull, || {
        read = true;
        reported
    });
    assert!(
        read != pull.merged,
        "the report was read after a merge, or not read"
    );
    assert!(
        pull.merged == (state == QueueState::Merged),
        "Merged without a merge record, or a merge record not Merged"
    );
    if !pull.merged && reported == Some(QueueState::Merged) {
        assert!(
            state == QueueState::Unknown,
            "an unrecorded merge report is not Unknown"
        );
    }
    kani::cover!(
        pull.merged && reported == Some(QueueState::FailedCi),
        "the merge record wins over a failure report"
    );
    kani::cover!(
        !pull.merged && reported == Some(QueueState::Merged) && state == QueueState::Unknown,
        "a delayed merge report is Unknown"
    );
    kani::cover!(
        !pull.merged && reported.is_none() && state == QueueState::Unknown,
        "no queue check is Unknown"
    );
}

/// K-observe-conflict: a dequeue on a PR GitHub says can't merge, or whose
/// state is dirty, is a conflict; any other report passes through unchanged.
/// A pending mergeability means GitHub has said nothing either way.
fn check_observe_conflict() {
    let pull: PullView = kani::any();
    let reported: Option<QueueState> = kani::any();
    let state = queue_state(pull, || reported);
    if !pull.merged && reported == Some(QueueState::Dequeued) {
        let conflicting = pull.mergeable == Some(false) || pull.mergeable_state == Mergeable::Dirty;
        assert!(
            state
                == if conflicting {
                    QueueState::Conflict
                } else {
                    QueueState::Dequeued
                },
            "a dequeue is misclassified"
        );
    }
    if !pull.merged && !matches!(reported, Some(QueueState::Merged | QueueState::Dequeued)) {
        assert!(
            state == reported.unwrap_or(QueueState::Unknown),
            "a report changed"
        );
    }
    if mergeability_pending(state, pull) {
        assert!(
            pull.mergeable.is_none()
                && matches!(
                    pull.mergeable_state,
                    Mergeable::Missing | Mergeable::Unknown
                ),
            "pending despite a known mergeability"
        );
    }
    kani::cover!(
        state == QueueState::Conflict && pull.mergeable == Some(true),
        "a dirty PR conflicts though GitHub calls it mergeable"
    );
    kani::cover!(
        state == QueueState::Conflict && pull.mergeable == Some(false),
        "an unmergeable PR conflicts"
    );
    kani::cover!(state == QueueState::Dequeued, "a plain dequeue");
    kani::cover!(mergeability_pending(state, pull), "mergeability pending");
}

/// K-observe-rule: the queue rule concluded exactly when there is a selected
/// queue check and the newest rule check from Mergify on the head completed
/// with a conclusion, no earlier than the queue check.
fn check_observe_rule_concluded() {
    let (checks, len) = any_checks();
    let checks = &checks[..len];
    let queue: Option<usize> = kani::any();
    kani::assume(queue.is_none_or(|queue| queue < len));
    let concluded = rule_concluded(checks, queue);
    let mut expected = false;
    if let Some(queue) = queue {
        let mut r = 0;
        while r < len {
            let rule = &checks[r];
            if newest_last(checks, r, Name::QueueRule)
                && rule.status == Status::Completed
                && rule.conclusion.is_some()
                && rule.completed_at.is_some()
                && checks[queue].completed_at.is_some()
                && rule.completed_at >= checks[queue].completed_at
            {
                expected = true;
            }
            r += 1;
        }
    }
    assert!(concluded == expected, "rule conclusion misjudged");
    kani::cover!(concluded, "the rule concluded");
    kani::cover!(
        !concluded
            && queue.is_some()
            && len == 2
            && checks[0].completed_at == checks[1].completed_at
            && checks[0].completed_at.is_some(),
        "the same time is not enough without a concluded rule"
    );
    kani::cover!(
        queue == Some(0)
            && len == 2
            && newest_last(checks, 1, Name::QueueRule)
            && checks[1].completed()
            && checks[1].conclusion.is_some()
            && checks[1].completed_at < checks[0].completed_at
            && checks[1].completed_at.is_some(),
        "a rule that finished before the dequeue does not count"
    );
}

/// K-observe-state: a timeout or a runner that never started fails the
/// queue's CI without reading the report, and is infrastructure; otherwise
/// the report decides. A merge through the queue needs a successful,
/// completed queue check that reports Merged.
fn check_observe_check_state() {
    let conclusion: Option<Conclusion> = kani::any();
    let report: QueueState = kani::any();
    let fails = matches!(
        conclusion,
        Some(Conclusion::TimedOut | Conclusion::StartupFailure)
    );
    let mut read = false;
    let state = check_state(conclusion, || {
        read = true;
        report
    });
    assert!(
        state == if fails { QueueState::FailedCi } else { report },
        "check state misjudged"
    );
    assert!(
        read != fails,
        "the report was read when it can't matter, or not read"
    );
    let report_infra: bool = kani::any();
    assert!(
        ci_is_infrastructure(conclusion, || report_infra) == (fails || report_infra),
        "infrastructure misjudged"
    );
    let check: CheckView = kani::any();
    let via = merged_via_queue(&check, || check_state(check.conclusion, || report));
    assert!(
        via == (check.completed()
            && check.conclusion == Some(Conclusion::Success)
            && report == QueueState::Merged),
        "merge through the queue misjudged"
    );
    kani::cover!(
        fails && report == QueueState::Queued,
        "a timeout overrides a queued report"
    );
    kani::cover!(
        !fails && state == QueueState::Conflict,
        "the report decides"
    );
    kani::cover!(via, "merged through the queue");
    kani::cover!(
        !via && check.completed() && report == QueueState::Merged,
        "a merged report without success"
    );
}

/// K-checks-passed: the merge commit's checks passed exactly when the
/// adapter's former `BTreeMap` grouping by app and name keeps at least one
/// check, and every kept check completed with success, skipped or neutral.
fn check_checks_passed() {
    let (checks, len) = any_checks();
    let checks = &checks[..len];
    let (newest, n) = btreemap_newest(checks, same_app_and_name);
    let mut i = 0;
    while i < len {
        assert!(
            newest_first(checks, i, |c| c.head == Head::Current, same_app_and_name)
                == kept(&newest, n, i),
            "grouping differs from the BTreeMap"
        );
        i += 1;
    }
    let mut all = n > 0;
    let mut k = 0;
    while k < n {
        let check = &checks[newest[k]];
        if !check.completed() || !check.conclusion.is_some_and(Conclusion::passing) {
            all = false;
        }
        k += 1;
    }
    let passed = checks_passed(checks);
    assert!(passed == all, "checks_passed misjudged");
    kani::cover!(
        passed
            && len == 2
            && same_app_and_name(&checks[0], &checks[1])
            && checks[0].head == Head::Current
            && checks[0].conclusion == Some(Conclusion::Failure),
        "a newer pass supersedes an older failure"
    );
    kani::cover!(
        !passed
            && len == 2
            && checks[0].name_key == checks[1].name_key
            && checks[0].app_key != checks[1].app_key
            && checks[0].conclusion == Some(Conclusion::Success),
        "the same name from another app counts on its own"
    );
    kani::cover!(!passed && len > 0 && n == 0, "no check on the merge commit");
    kani::cover!(passed && n == 2, "two passing groups");
}

/// K-queue-blocked: a queue is blocked exactly when Mergify waits, every
/// suite completed and conditions are unmet. The failures it names are the
/// newest check per name on the head (the former `BTreeMap` by name) that
/// completed with a failing conclusion and are neither Mergify's nor
/// required. The head's checks are settled when every non-Mergify one on it
/// completed.
fn check_queue_blocked() {
    let state: QueueState = kani::any();
    let suites: bool = kani::any();
    let unmet: bool = kani::any();
    assert!(
        queue_blocked(state, suites, unmet) == (state == QueueState::Waiting && suites && unmet),
        "blocked misjudged"
    );
    let (checks, len) = any_checks();
    let checks = &checks[..len];
    let (newest, n) = btreemap_newest(checks, same_name);
    let mut failures = 0;
    let mut settled = true;
    let mut i = 0;
    while i < len {
        let check = &checks[i];
        let expected = kept(&newest, n, i)
            && check.completed()
            && matches!(
                check.conclusion,
                Some(
                    Conclusion::Failure
                        | Conclusion::Cancelled
                        | Conclusion::TimedOut
                        | Conclusion::ActionRequired
                        | Conclusion::StartupFailure
                )
            )
            && !check.app.mergify_slug()
            && !check.required;
        let failing = blocking_failure(checks, i);
        assert!(failing == expected, "blocking failure misjudged");
        if failing {
            failures += 1;
        }
        if check.head == Head::Current && !check.app.mergify_slug() && !check.completed() {
            settled = false;
        }
        i += 1;
    }
    assert!(checks_settled(checks) == settled, "settled misjudged");
    kani::cover!(failures == 2, "two failing names");
    kani::cover!(
        failures == 0
            && len == 1
            && checks[0].head == Head::Current
            && checks[0].completed()
            && checks[0].conclusion == Some(Conclusion::Failure)
            && checks[0].required,
        "a required failure is not named"
    );
    kani::cover!(
        failures == 0
            && len == 2
            && same_name(&checks[0], &checks[1])
            && checks[0].conclusion == Some(Conclusion::Failure)
            && checks[0].head == Head::Current
            && checks[1].head == Head::Current
            && checks[1].id > checks[0].id,
        "a newer run supersedes a failure"
    );
    kani::cover!(
        !settled && len == 1 && checks[0].app == App::Actions,
        "a running check leaves the head unsettled"
    );
    kani::cover!(
        settled && len == 1 && checks[0].app == App::MergifySlug && !checks[0].completed(),
        "Mergify's own running check does not count"
    );
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
    let stale = kind.names_attempt_head() && !head_current;
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
            None => assert!(kind.setup(), "a non-setup fact missed the kernel"),
        }
    }
    kani::cover!(stale, "a stale attempt is recorded");
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

/// http_status_class_total: every status code, with or without GitHub's
/// rate-limit signal, has exactly one class, as the table states.
fn check_http_status_class_total() {
    let status: u16 = kani::any();
    let signal: bool = kani::any();
    let class = classify(status, signal);
    let expected = match status {
        429 => StatusClass::Quota,
        403 if signal => StatusClass::Quota,
        401 | 403 => StatusClass::Permission,
        404 | 410 => StatusClass::NotFound,
        422 => StatusClass::InvalidRequest,
        500..=599 => StatusClass::UpstreamServer,
        _ => StatusClass::Transport,
    };
    assert!(class == expected, "status misclassified");
    assert!(
        rate_limited(status, signal) == (class == StatusClass::Quota),
        "rate limit and quota disagree"
    );
    assert!(
        status_class(status) != StatusClass::Quota,
        "a plain error response is quota"
    );
    kani::cover!(
        class == StatusClass::Quota && status == 403,
        "a 403 rate limit"
    );
    kani::cover!(
        class == StatusClass::Permission && status == 403,
        "a 403 permission"
    );
    kani::cover!(class == StatusClass::NotFound && status == 410, "gone");
    kani::cover!(class == StatusClass::InvalidRequest, "invalid request");
    kani::cover!(class == StatusClass::UpstreamServer, "server error");
    kani::cover!(
        class == StatusClass::Transport && status == 400,
        "another status"
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

proof!(observe_select, 4, check_observe_select());
proof!(observe_merged, 4, check_observe_merged());
proof!(observe_conflict, 4, check_observe_conflict());
proof!(observe_rule_concluded, 4, check_observe_rule_concluded());
proof!(observe_check_state, 4, check_observe_check_state());
proof!(checks_passed_total, 4, check_checks_passed());
proof!(queue_blocked_total, 4, check_queue_blocked());
proof!(q52, 4, check_q52());
proof!(translate_closed, 4, check_translate_closed());
proof!(translate_head_ci, 4, check_translate_head_ci());
proof!(translate_route, 4, check_translate_route());
proof!(ci_classify, 4, check_ci_classify());
proof!(person_facts_total, 4, check_person_facts());
proof!(http_status_class_total, 4, check_http_status_class_total());
proof!(facts_outcome, 4, check_facts_outcome());
