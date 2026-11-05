use super::*;
use tl3_kernel::{EffectKind, InFlight, Outcome, QKind, Question, RepoId, Resume, Role};

const NOW: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

fn request(id: u64, phase: Phase) -> Request {
    let mut node = Node::new(RepoId(0));
    node.phase = phase;
    Request {
        direct_delivery: false,
        id: NodeId(id),
        node,
        repo: "RobTand/tl3-sandbox".into(),
        issue: 40 + id,
        title: format!("Request {id}"),
        question_post_failed: false,
        phase_since_ms: Some(NOW - 5 * MINUTE),
        progress: None,
        worker_failure: None,
        budget_exhaustion: None,
        incidents: vec![],
        maker_model: None,
        question_context: None,
    }
}

fn board(requests: Vec<Request>) -> Board {
    project(Input {
        revision: 7,
        now_ms: NOW,
        requests,
        unassigned_incidents: vec![],
        models: None,
        quota_resets: BTreeMap::new(),
    })
}

fn card(request: Request) -> Card {
    board(vec![request]).cards.remove(0)
}

const OUTCOMES: [(Outcome, Ending, &str); 6] = [
    (Outcome::Merged, Ending::Merged, "merged"),
    (Outcome::Stopped, Ending::Stopped, "stopped"),
    (Outcome::Replaced, Ending::Replaced, "replaced"),
    (Outcome::Duplicate, Ending::Duplicate, "duplicate"),
    (Outcome::AlreadyDone, Ending::AlreadyDone, "already done"),
    (
        Outcome::NotActionable,
        Ending::NotActionable,
        "not actionable",
    ),
];

#[test]
fn every_phase_maps_to_its_column_and_label() {
    let cases = [
        (
            Phase::Ingesting,
            Column::Intake,
            "Understanding the request",
        ),
        (Phase::Auditing, Column::Intake, "Auditing the goals"),
        (Phase::Unplanned, Column::Planning, "Waiting for a plan"),
        (
            Phase::Planning { must_split: false },
            Column::Planning,
            "Planning",
        ),
        (
            Phase::Planning { must_split: true },
            Column::Planning,
            "Planning",
        ),
        (Phase::Ready(Resume::Making), Column::Making, "Ready"),
        (Phase::Ready(Resume::Checking), Column::Checking, "Ready"),
        (Phase::Ready(Resume::Reviewing), Column::Reviewing, "Ready"),
        (Phase::Ready(Resume::Publishing), Column::Merging, "Ready"),
        (Phase::Ready(Resume::Rebasing), Column::Merging, "Ready"),
        (Phase::Making, Column::Making, "Making the change"),
        (Phase::Checking, Column::Checking, "Running checks"),
        (Phase::Reviewing, Column::Reviewing, "Reviewing"),
        (Phase::Publishing, Column::Merging, "Publishing"),
        (Phase::Queued, Column::Merging, "In the merge queue"),
        (Phase::Rebasing, Column::Merging, "Updating the branch"),
        (Phase::AwaitingRob, Column::NeedsRob, "Awaiting Rob"),
    ];
    for (phase, column, label) in cases {
        assert_eq!(Column::of(phase), column, "{phase:?}");
        let card = card(request(0, phase));
        assert_eq!(
            (card.column, card.phase.as_str(), card.ending),
            (column, label, None),
            "{phase:?}"
        );
    }
}

/// Rob, 2026-11-02: stopped work isn't done, and cancelled work isn't a merge.
const OUTCOME_COLUMNS: [(Outcome, Column); 6] = [
    (Outcome::Merged, Column::Done),
    (Outcome::AlreadyDone, Column::Done),
    (Outcome::Replaced, Column::Done),
    (Outcome::Stopped, Column::Backlog),
    (Outcome::Duplicate, Column::Cancelled),
    (Outcome::NotActionable, Column::Cancelled),
];

#[test]
fn every_outcome_lands_in_its_column() {
    for (outcome, column) in OUTCOME_COLUMNS {
        assert_eq!(Column::of(Phase::Closed(outcome)), column, "{outcome:?}");
        assert_eq!(Column::of(Phase::Stopping(outcome)), column, "{outcome:?}");
    }
}

#[test]
fn every_outcome_keeps_its_ending_and_phase_label() {
    for (outcome, ending, label) in OUTCOMES {
        let closed = card(request(0, Phase::Closed(outcome)));
        assert_eq!(closed.ending, Some(ending));
        assert_eq!(closed.phase, format!("Closed: {label}"));
        let stopping = card(request(0, Phase::Stopping(outcome)));
        assert_eq!(stopping.ending, Some(ending));
        assert_eq!(stopping.phase, "Cleaning up");
        let column = OUTCOME_COLUMNS
            .iter()
            .find(|(candidate, _)| *candidate == outcome)
            .unwrap()
            .1;
        assert_eq!(closed.column, column);
        assert_eq!(
            stopping.column, column,
            "a stop still cleaning up maps by its outcome"
        );
    }
}

#[test]
fn only_stopped_cards_carry_the_stopped_badge() {
    for (outcome, ..) in OUTCOMES {
        let expected: Vec<Badge> = if outcome == Outcome::Stopped {
            vec![Badge::Stopped]
        } else {
            vec![]
        };
        assert_eq!(
            card(request(0, Phase::Closed(outcome))).badges,
            expected,
            "{outcome:?}"
        );
        assert_eq!(
            card(request(0, Phase::Stopping(outcome))).badges,
            expected,
            "{outcome:?} while cleaning up"
        );
    }
    assert_eq!(card(request(0, Phase::Making)).badges, vec![]);
}

#[test]
fn columns_are_in_board_order_with_labels() {
    let labels: Vec<_> = board(vec![])
        .flow
        .columns
        .into_iter()
        .map(|count| (count.column, count.label, count.count))
        .collect();
    assert_eq!(
        labels,
        [
            (Column::Backlog, "Backlog", 0),
            (Column::Intake, "Intake", 0),
            (Column::Planning, "Planning", 0),
            (Column::Making, "Making", 0),
            (Column::Checking, "Checking", 0),
            (Column::Reviewing, "Reviewing", 0),
            (Column::Merging, "Merging", 0),
            (Column::NeedsRob, "Needs Rob", 0),
            (Column::Done, "Done", 0),
            (Column::Cancelled, "Cancelled", 0),
        ]
        .map(|(column, label, count)| (column, label.to_string(), count))
    );
}

#[test]
fn badges_mark_problems() {
    let mut paused = request(0, Phase::Making);
    paused.node.paused = true;
    let mut incident = request(1, Phase::Checking);
    incident.node.waiting_incident = true;
    let mut blocked = request(2, Phase::Making);
    blocked.incidents.push(StoredIncident {
        id: "inc-1".into(),
        summary: "PrismaBuild action abc ended in a test infrastructure error".into(),
        maker_blocked: true,
    });
    let mut question = request(3, Phase::Planning { must_split: false });
    question.node.question = Some(Question {
        id: 9,
        kind: QKind::CantMeet,
        cont: tl3_kernel::Cont::Nowhere,
        cost: 0,
        clarified: false,
    });
    question.node.run = Some(1);
    let awaiting = request(4, Phase::AwaitingRob);
    let mut needs_rob = request(5, Phase::Reviewing);
    needs_rob.node.incident_needs_rob = true;
    needs_rob.node.run = Some(1);
    let planner_wait = request(6, Phase::Planning { must_split: true });
    let slot_wait = request(7, Phase::Ready(Resume::Making));
    let mut working = request(8, Phase::Making);
    working.node.run = Some(3);
    let badges: Vec<_> = board(vec![
        paused,
        incident,
        blocked,
        question,
        awaiting,
        needs_rob,
        planner_wait,
        slot_wait,
        working,
    ])
    .cards
    .into_iter()
    .map(|card| card.badges)
    .collect();
    assert_eq!(
        badges,
        vec![
            vec![Badge::Paused],
            vec![Badge::Incident],
            vec![Badge::Incident, Badge::MakerBlocked],
            vec![Badge::NeedsRob],
            vec![Badge::NeedsRob],
            vec![Badge::NeedsRob],
            vec![Badge::WaitingForCapacity],
            vec![Badge::WaitingForCapacity],
            vec![],
        ]
    );
}

#[test]
fn card_carries_counters_progress_and_incidents() {
    let config = tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
    let mut making = request(3, Phase::Making);
    making.title = "Fix\u{202e} the <b>build</b>\n\tnow".into();
    making.node.attempt = 3;
    making.node.round = 2;
    making.node.plan_retries = 1;
    making.node.audit_rounds = 2;
    making.node.run_failures = 1;
    making.node.fx_failures = 4;
    making.node.credit.ready = true;
    making.node.credit.start = 6;
    making.node.credit.spent = 2;
    making.node.run = Some(5);
    making.progress = Some(Progress {
        at_ms: 1_700_000_000_000,
        model_turn: true,
    });
    making.worker_failure = Some(WorkerFailure {
        class: "ValueError".into(),
        message: "bad reply".into(),
    });
    making.incidents.push(StoredIncident {
        id: "inc-7".into(),
        summary: "Checks stalled".into(),
        maker_blocked: false,
    });
    let mut reviewing = request(4, Phase::Reviewing);
    reviewing.node.attempt = 1;
    reviewing.node.run = Some(2);
    let mut planning = request(5, Phase::Planning { must_split: false });
    planning.node.plan_retries = 1;
    planning.node.run = Some(2);
    let checking = request(6, Phase::Checking);
    let mut untitled = request(7, Phase::Ingesting);
    untitled.title = " \u{200b} ".into();
    untitled.phase_since_ms = None;
    let cards = project(Input {
        revision: 7,
        now_ms: NOW,
        requests: vec![making, reviewing, planning, checking, untitled],
        unassigned_incidents: vec![],
        models: Some(&config.models),
        quota_resets: BTreeMap::new(),
    })
    .cards;
    assert_eq!(
        cards[0],
        Card {
            id: 3,
            repo: "RobTand/tl3-sandbox".into(),
            issue: 43,
            title: "Fix the <b>build</b> now".into(),
            column: Column::Making,
            phase: "Making the change".into(),
            ending: None,
            waiting_on: "the current task to finish".into(),
            priority: "unranked".into(),
            pool_position: None,
            role: Some("Maker".into()),
            model: Some("alternate-model".into()),
            attempt: 3,
            round: 2,
            retries: Retries {
                plan: 1,
                audit: 2,
                run_failures: 1,
                effect_failures: 4,
            },
            credit: Some(4),
            phase_since_ms: Some(NOW - 5 * MINUTE),
            last_progress: Some(
                "Model turn completed at 2023-11-14 22:13:20 UTC; last worker failure: ValueError: bad reply"
                    .into()
            ),
            incidents: vec![CardIncident {
                id: "inc-7".into(),
                summary: "Checks stalled".into(),
            }],
            badges: vec![Badge::Incident],
            question: None,
        }
    );
    let roles: Vec<_> = cards
        .iter()
        .map(|card| (card.role.as_deref(), card.model.as_deref()))
        .collect();
    assert_eq!(
        roles,
        [
            (Some("Maker"), Some("alternate-model")),
            // Attempt 1's maker is the default family, so review takes the alternate.
            (Some("Reviewer"), Some("alternate-model")),
            (Some("Planner"), Some("alternate-model")),
            (None, None),
            (Some("Reader"), Some("default-model")),
        ]
    );
    assert_eq!(cards[4].title, "title not recorded");
    assert_eq!(
        cards[4].credit, None,
        "credit is set up when ingest finishes"
    );
    assert_eq!(cards[4].phase_since_ms, None);
    assert_eq!(
        card(request(0, Phase::Making)).last_progress,
        None,
        "no progress and no failure"
    );
    let mut failed_only = request(0, Phase::Making);
    failed_only.worker_failure = Some(WorkerFailure {
        class: "Timeout".into(),
        message: "no reply".into(),
    });
    assert_eq!(
        card(failed_only).last_progress.as_deref(),
        Some("none recorded; last worker failure: Timeout: no reply")
    );
}

#[test]
fn waiting_on_names_the_next_evidence() {
    let mut failed_question = request(0, Phase::Planning { must_split: false });
    failed_question.question_post_failed = true;
    let mut effect = request(1, Phase::Making);
    effect.node.effect = Some(InFlight {
        seq: 1,
        kind: EffectKind::StartRun(Role::Make),
        cancelling: false,
    });
    let mut checks = request(2, Phase::Checking);
    checks.node.job = Some(1);
    let waits: Vec<_> = board(vec![
        failed_question,
        effect,
        checks,
        request(3, Phase::Queued),
        request(4, Phase::Closed(Outcome::Merged)),
        request(5, Phase::Rebasing),
    ])
    .cards
    .into_iter()
    .map(|card| card.waiting_on)
    .collect();
    assert_eq!(
        waits,
        [
            "the question post that failed; the question was never posted",
            "writer to start",
            "check results",
            "merge queue",
            "none",
            "the branch update to finish",
        ]
    );
}

#[test]
fn flow_counts_stages_flight_and_recent_merges() {
    let mut merged_now = request(0, Phase::Closed(Outcome::Merged));
    merged_now.phase_since_ms = Some(NOW - 10 * MINUTE);
    let mut merged_today = request(1, Phase::Closed(Outcome::Merged));
    merged_today.phase_since_ms = Some(NOW - 5 * 60 * MINUTE);
    let mut merged_long_ago = request(2, Phase::Closed(Outcome::Merged));
    merged_long_ago.phase_since_ms = Some(NOW - 25 * 60 * MINUTE);
    let mut stopped_now = request(3, Phase::Closed(Outcome::Stopped));
    stopped_now.phase_since_ms = Some(NOW - MINUTE);
    let mut duplicate_now = request(9, Phase::Closed(Outcome::Duplicate));
    duplicate_now.phase_since_ms = Some(NOW - MINUTE);
    let flow = board(vec![
        merged_now,
        merged_today,
        merged_long_ago,
        stopped_now,
        request(4, Phase::Stopping(Outcome::Merged)),
        request(5, Phase::Ingesting),
        request(6, Phase::Auditing),
        request(7, Phase::Making),
        request(8, Phase::AwaitingRob),
        duplicate_now,
        request(10, Phase::Stopping(Outcome::Stopped)),
    ])
    .flow;
    let counts: Vec<_> = flow
        .columns
        .iter()
        .map(|count| (count.column, count.count))
        .filter(|(_, count)| *count > 0)
        .collect();
    assert_eq!(
        counts,
        [
            (Column::Backlog, 2),
            (Column::Intake, 2),
            (Column::Making, 1),
            (Column::NeedsRob, 1),
            (Column::Done, 4),
            (Column::Cancelled, 1),
        ]
    );
    // Stopped and cancelled work isn't in flight.
    assert_eq!(flow.in_flight, 4);
    // The merge that is still cleaning up counts too.
    assert_eq!(flow.merged_last_hour, 2);
    assert_eq!(flow.merged_last_day, 3);
}

#[test]
fn board_keeps_revision_order_and_unassigned_incidents() {
    let board = project(Input {
        revision: 12,
        now_ms: NOW,
        requests: vec![request(4, Phase::Making), request(2, Phase::Checking)],
        unassigned_incidents: vec![StoredIncident {
            id: "inc-9".into(),
            summary: "Hindsight rejected the key".into(),
            maker_blocked: false,
        }],
        models: None,
        quota_resets: BTreeMap::new(),
    });
    assert_eq!(board.revision, 12);
    assert_eq!(board.built_at_ms, NOW);
    assert_eq!(
        board.cards.iter().map(|card| card.id).collect::<Vec<_>>(),
        [2, 4]
    );
    assert_eq!(
        board.unassigned_incidents,
        [CardIncident {
            id: "inc-9".into(),
            summary: "Hindsight rejected the key".into()
        }]
    );
}

#[test]
fn patch_carries_only_changed_cards() {
    let before = board(vec![
        request(0, Phase::Making),
        request(1, Phase::Checking),
        request(2, Phase::Reviewing),
    ]);
    let mut moved = request(1, Phase::Reviewing);
    moved.phase_since_ms = Some(NOW);
    let mut after = board(vec![
        request(0, Phase::Making),
        moved,
        request(3, Phase::Ingesting),
    ]);
    after.revision = 8;
    let patch = after.patch_since(&before);
    assert_eq!(patch.revision, 8);
    assert_eq!(patch.flow, after.flow);
    assert_eq!(
        patch.upserts.iter().map(|card| card.id).collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(patch.upserts[0].column, Column::Reviewing);
    assert_eq!(patch.removals, [2]);
    assert!(after.patch_since(&after).upserts.is_empty());
}

fn closed(id: u64, outcome: Outcome, age_ms: i64) -> Request {
    let mut request = request(id, Phase::Closed(outcome));
    request.phase_since_ms = Some(NOW - age_ms);
    request
}

const HOURS: i64 = 60 * MINUTE;
const DAYS: i64 = 24 * HOURS;

#[test]
fn done_and_cancelled_show_the_last_day_and_count_every_ending() {
    let mut unknown = request(2, Phase::Closed(Outcome::Duplicate));
    unknown.phase_since_ms = None;
    let mut cleaning = request(3, Phase::Stopping(Outcome::Merged));
    cleaning.phase_since_ms = Some(NOW - 30 * HOURS);
    let board = board(vec![
        closed(0, Outcome::Merged, 23 * HOURS),
        closed(1, Outcome::Merged, 25 * HOURS),
        unknown,
        cleaning,
        request(4, Phase::Making),
        closed(5, Outcome::AlreadyDone, 25 * HOURS),
        closed(6, Outcome::Duplicate, 23 * HOURS),
        closed(7, Outcome::NotActionable, 25 * HOURS),
        closed(8, Outcome::Replaced, 25 * HOURS),
    ]);
    assert_eq!(
        board.cards.iter().map(|card| card.id).collect::<Vec<_>>(),
        [0, 3, 4, 6],
        "done and cancelled requests older than a day leave the board; cleanup stays"
    );
    let count = |column| {
        board
            .flow
            .columns
            .iter()
            .find(|count| count.column == column)
            .unwrap()
            .count
    };
    assert_eq!(count(Column::Done), 5, "the count includes every ending");
    assert_eq!(
        count(Column::Cancelled),
        3,
        "the count includes every ending"
    );
}

#[test]
fn stopped_requests_wait_in_the_backlog_for_a_week() {
    let mut unknown = request(4, Phase::Closed(Outcome::Stopped));
    unknown.phase_since_ms = None;
    let mut cleaning = request(5, Phase::Stopping(Outcome::Stopped));
    cleaning.phase_since_ms = Some(NOW - 30 * DAYS);
    let board = board(vec![
        closed(0, Outcome::Stopped, 2 * HOURS),
        closed(1, Outcome::Stopped, 3 * DAYS),
        closed(2, Outcome::Stopped, 7 * DAYS - HOURS),
        closed(3, Outcome::Stopped, 7 * DAYS + HOURS),
        unknown,
        cleaning,
    ]);
    assert_eq!(
        board.cards.iter().map(|card| card.id).collect::<Vec<_>>(),
        [0, 1, 2, 5],
        "a stopped request leaves the backlog a week after it closed; cleanup stays"
    );
    assert!(
        board
            .cards
            .iter()
            .all(|card| card.column == Column::Backlog && card.badges == [Badge::Stopped])
    );
    let backlog = board
        .flow
        .columns
        .iter()
        .find(|count| count.column == Column::Backlog)
        .unwrap();
    assert_eq!(backlog.count, 4, "the backlog counts what it shows");
}

#[test]
fn only_merges_count_as_merged() {
    let board = board(vec![
        closed(0, Outcome::Merged, MINUTE),
        closed(1, Outcome::AlreadyDone, MINUTE),
        closed(2, Outcome::Stopped, MINUTE),
        closed(3, Outcome::Duplicate, MINUTE),
        closed(4, Outcome::NotActionable, MINUTE),
        closed(5, Outcome::Replaced, MINUTE),
    ]);
    assert_eq!(board.flow.merged_last_hour, 1);
    assert_eq!(board.flow.merged_last_day, 1);
    assert_eq!(board.flow.in_flight, 0);
}

#[test]
fn a_cancelled_step_is_not_the_wait() {
    // Stopping from the merge queue leaves the queue admission in flight, cancelled.
    let mut stopping = request(0, Phase::Stopping(Outcome::Stopped));
    stopping.node.effect = Some(InFlight {
        seq: 3,
        kind: EffectKind::Enqueue,
        cancelling: true,
    });
    let mut tearing_down = request(1, Phase::Stopping(Outcome::Stopped));
    tearing_down.node.effect = Some(InFlight {
        seq: 4,
        kind: EffectKind::StopRun,
        cancelling: false,
    });
    let mut moved_on = request(2, Phase::Ready(Resume::Making));
    moved_on.node.effect = Some(InFlight {
        seq: 1,
        kind: EffectKind::StartRun(Role::Make),
        cancelling: true,
    });
    let waits: Vec<_> = board(vec![stopping, tearing_down, moved_on])
        .cards
        .into_iter()
        .map(|card| card.waiting_on)
        .collect();
    assert_eq!(
        waits,
        [
            "a cancelled step to settle",
            "the current task to stop",
            "a cancelled step to settle",
        ]
    );
}

/// The card names the model the worker runs: Q53's quota fallback, and a
/// reviewer from another family than the maker that actually ran.
#[test]
fn card_names_the_model_the_worker_runs() {
    let config = tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
    let mut making = request(0, Phase::Making);
    making.node.attempt = 1;
    let mut reviewing = request(1, Phase::Reviewing);
    reviewing.node.attempt = 1;
    // The maker fell back to the alternate model, so review takes the default.
    reviewing.maker_model = Some("alternate-model".into());
    let models = |quota_resets: BTreeMap<String, u64>| -> Vec<_> {
        project(Input {
            revision: 1,
            now_ms: NOW,
            requests: vec![making.clone(), reviewing.clone()],
            unassigned_incidents: vec![],
            models: Some(&config.models),
            quota_resets,
        })
        .cards
        .into_iter()
        .map(|card| card.model)
        .collect()
    };
    assert_eq!(
        models(BTreeMap::new()),
        [Some("default-model".into()), Some("default-model".into())]
    );
    let out = BTreeMap::from([("default-model".to_string(), (NOW + 60 * MINUTE) as u64)]);
    assert_eq!(
        models(out),
        [Some("alternate-model".into()), Some("default-model".into())],
        "an out-of-quota model falls back down the ladder"
    );
    let reset = BTreeMap::from([("default-model".to_string(), (NOW - MINUTE) as u64)]);
    assert_eq!(
        models(reset)[0],
        Some("default-model".into()),
        "a past reset no longer limits"
    );
}

#[test]
fn pool_waiters_show_their_priority_and_place_in_line() {
    let mut older = request(1, Phase::Ingesting);
    older.node.ticket = Some(3);
    older.node.priority = tl3_kernel::Priority::P3;
    let mut better = request(2, Phase::Auditing);
    better.node.ticket = Some(5);
    better.node.priority = tl3_kernel::Priority::P1;
    let mut running = request(3, Phase::Making);
    running.node.priority = tl3_kernel::Priority::P0;
    let cards = board(vec![older, better, running]).cards;
    let shown: Vec<_> = cards
        .iter()
        .map(|card| {
            (
                card.priority.as_str(),
                card.pool_position,
                card.waiting_on.as_str(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("P3", Some(2), "an agent slot (P3, 2nd in line)"),
            ("P1", Some(1), "an agent slot (P1, 1st in line)"),
            ("P0", None, "the change to be written"),
        ]
    );
    assert!(cards[0].badges.contains(&Badge::WaitingForCapacity));
}
