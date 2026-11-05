use super::*;

fn set_issue(repo: &Repo, number: u64, state: &str, reason: Option<&str>) {
    crate::adapters::github::set_fake_issue(
        &repo.root.path().join("fakes"),
        &serde_json::from_value(json!({"number":number,"title":"Prerequisite","body":null,
            "state":state,"state_reason":reason,"updated_at":"2026-11-03T23:00:00Z"}))
        .unwrap(),
    );
}

fn status(settings: &tl3_config::Store) -> crate::status::Status {
    crate::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap()
}

fn reopen(repo: &Repo, c: Controller, settings: &tl3_config::Store) -> Controller {
    let bootstrap = c.bootstrap.clone();
    drop(c);
    let mut c = Controller::open(
        tl3_store::Store::open(settings).unwrap(),
        crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
        bootstrap,
    )
    .unwrap();
    for _ in 0..64 {
        if !c
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "inbox")
        {
            return c;
        }
        assert!(c.drive_once().unwrap());
    }
    panic!("startup prerequisite reads did not drain");
}

/// #839 / Q27, Rob 2026-11-05: each closed prerequisite releases its own successor.
#[test]
fn closed_root_regressions_release_nine_and_four_waiter_chains_in_order() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_capacity(&mut db, "t839_chains", 32, 32);
    for (root, count) in [(2763, 9), (2773, 4), (2779, 1), (2781, 1)] {
        set_issue(&repo, root, "open", None);
        let root_node = intake(&mut c, &repository, root);
        let mut waiters = Vec::new();
        for number in root + 1..=root + count {
            set_issue(&repo, number, "open", None);
            let node = intake(&mut c, &repository, number);
            ingest(&mut c, node, &[number - 1]);
            waiters.push(node);
        }
        reply(
            &mut c,
            root_node,
            Reply::IngestEarlyEnd(EarlyEnd::NotActionable),
        );
        reply(&mut c, root_node, Reply::AuditAgrees);
        assert_eq!(
            c.world().unwrap().nodes[root_node.0 as usize].phase,
            Phase::Closed(Outcome::NotActionable)
        );
        assert!(
            waiters
                .iter()
                .all(|node| c.world().unwrap().nodes[node.0 as usize].phase == Phase::Unplanned)
        );
        for (index, &node) in waiters.iter().enumerate() {
            let predecessor = root + index as u64;
            set_issue(&repo, predecessor, "closed", Some("not_planned"));
            external(
                &mut c,
                &repository,
                predecessor,
                DependencyStatus::Abandoned,
            );
            assert_eq!(
                c.world().unwrap().nodes[node.0 as usize].phase,
                Phase::Planning { must_split: false }
            );
            for &later in &waiters[index + 1..] {
                assert_eq!(
                    c.world().unwrap().nodes[later.0 as usize].phase,
                    Phase::Unplanned
                );
            }
            assert_eq!(
                super::super::closures(&c.snapshot().unwrap(), node).unwrap()[0].reason,
                "not_planned"
            );
            assert_eq!(
                c.prerequisite_evidence(node).unwrap()[0].state,
                PrerequisiteState::ClosedUnmerged {
                    reason: ClosedPrerequisiteReason::NotPlanned
                }
            );
        }
    }
    assert!(!crate::status::render_text(&status(&settings)).contains("waiting for"));
}

/// #839 / Q49: a stopped request is open, restartable, and remains pending.
#[test]
fn stopped_root_status_names_the_restartable_wait() {
    let mut db = Database::start();
    let (_repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_stopped");
    let root = intake(&mut c, &repository, 2761);
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    event(
        &mut c,
        Event::Command {
            node: root,
            id: 1,
            command: Command::Stop,
        },
    );
    confirm(&mut c, root);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        status(&settings)
            .requests
            .iter()
            .find(|r| r.issue.contains("#42:"))
            .unwrap()
            .waiting_on,
        "waiting for #2761: stopped"
    );
}

/// #839 / Q59: issues closed as completed continue to deliver prerequisites.
#[test]
fn completed_issue_delivers_without_a_closed_unmerged_wait() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_delivered");
    set_issue(&repo, 2761, "closed", Some("completed"));
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
    assert!(!crate::status::render_text(&status(&settings)).contains("closed without merge"));
}

/// #839: closures predating deploy are recovered once at controller open.
#[test]
fn restart_reads_already_closed_prerequisite_and_retains_it() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_restart");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    set_issue(&repo, 2761, "closed", Some("not_planned"));
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    set_issue(&repo, 2761, "open", None);
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
}

/// #839 / Q59: both status front ends project the same terminal prerequisite.
#[test]
fn caught_up_closure_has_identical_text_and_html_status() {
    let mut db = Database::start();
    let (_repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_status");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    c.observe(
        "t839-catch-up",
        Observation::Prerequisite(ExternalFact {
            updated_at: None,
            reason: None,
            repository: repository.clone(),
            kind: PrerequisiteKind::Issue,
            number: 2761,
            status: DependencyStatus::Abandoned,
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    let model = status(&settings);
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
    assert!(!crate::status::render_text(&model).contains("waiting for #2761"));
    assert!(!crate::status::render_html(&model).contains("waiting for #2761"));
}

/// #839 / Q27: a closed unmerged PR releases planning with retained closure evidence.
#[test]
fn closed_pr_delivers_and_retains_its_reason() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_pr");
    crate::adapters::github::set_fake_issue(
        &repo.root.path().join("fakes"),
        &serde_json::from_value(json!({"number":2761,"title":"Prerequisite PR","body":null,
            "state":"open","updated_at":"2026-11-03T23:00:00Z","pull_request":{"url":"https://api.github.com/repos/owner/sandbox/pulls/2761"}}))
        .unwrap(),
    );
    let waiter = intake(&mut c, &repository, 42);
    ingest_brief_kind(&mut c, waiter, &[2761], "pull_request");
    reply(&mut c, waiter, Reply::AuditAgrees);
    c.observe(
        "t839-pr-closed",
        Observation::Prerequisite(ExternalFact {
            updated_at: None,
            reason: None,
            repository: repository.clone(),
            kind: PrerequisiteKind::PullRequest,
            number: 2761,
            status: DependencyStatus::Abandoned,
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
    assert_eq!(
        c.prerequisite_evidence(waiter).unwrap()[0].state,
        PrerequisiteState::ClosedUnmerged {
            reason: ClosedPrerequisiteReason::PullRequestClosed
        }
    );
    assert!(
        !status(&settings).requests[0]
            .waiting_on
            .contains("waiting for")
    );
    c.observe(
        "t839-pr-merged-later",
        Observation::Prerequisite(ExternalFact {
            updated_at: None,
            reason: None,
            repository,
            kind: PrerequisiteKind::PullRequest,
            number: 2761,
            status: DependencyStatus::Delivered,
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
}

struct CountReads {
    github: crate::adapters::github::Fake,
    reads: std::rc::Rc<std::cell::Cell<usize>>,
}

impl executor::Executor for CountReads {
    fn prerequisite(
        &mut self,
        repo: &Repository,
        kind: PrerequisiteKind,
        number: u64,
    ) -> Result<DependencyStatus> {
        self.reads.set(self.reads.get() + 1);
        self.github.prerequisite(repo, kind, number)
    }
    fn execute(&mut self, delivery: &executor::Delivery) -> Result<executor::Completion> {
        self.github.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &executor::Delivery) -> Result<executor::Reconciliation> {
        self.github.reconcile(delivery)
    }
}

/// #839 / Q59: startup deduplicates identities and looks consume stored facts.
#[test]
fn shared_prerequisite_has_one_startup_read_and_no_look_polling() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_bounded");
    let mut waiters = Vec::new();
    for number in [42, 2772, 2779] {
        let node = intake(&mut c, &repository, number);
        ingest(&mut c, node, &[2761]);
        waiters.push(node);
    }
    let bootstrap = c.bootstrap.clone();
    drop(c);
    let reads = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut adapters = crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap();
    adapters.register(
        executor::Upstream::GitHub,
        CountReads {
            github: crate::adapters::github::Fake::new(&repo.root.path().join("fakes")),
            reads: reads.clone(),
        },
    );
    let mut c = Controller::open(
        tl3_store::Store::open(&settings).unwrap(),
        adapters,
        bootstrap,
    )
    .unwrap();
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "inbox")
            .count(),
        1
    );
    assert!(c.drive_once().unwrap());
    assert_eq!(reads.get(), 1);
    for node in waiters {
        assert_eq!(
            c.prerequisite_evidence(node).unwrap()[0].state,
            PrerequisiteState::Open
        );
    }
    c.look_now().unwrap();
    c.look_now().unwrap();
    assert_eq!(reads.get(), 1, "looks must never poll prerequisites");
}

/// #839 / Q27: a pre-deploy Abandoned kernel edge must not remain terminal.
#[test]
fn startup_reconciles_legacy_abandoned_edge_without_restart_command() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_legacy");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    let target = c.world().unwrap().dependencies()[0].target;
    event(
        &mut c,
        Event::Fact {
            node: waiter,
            fact: Fact::Dependency {
                target,
                status: DependencyStatus::Abandoned,
            },
        },
    );
    let snapshot = c.snapshot().unwrap();
    let mut seed = Commit::new("pre-839-terminal-fact", json!(null), &snapshot);
    seed.rows.push(put("prerequisite_facts", fact_key(&repository, PrerequisiteKind::Issue, 2761),
        json!({"repository": repository, "kind":PrerequisiteKind::Issue, "number":2761, "status":DependencyStatus::Abandoned})));
    c.store.commit(&seed).unwrap();
    set_issue(&repo, 2761, "closed", Some("not_planned"));
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
}

/// #839 / Q59: merged PRs satisfy the same controller path without missing-code evidence.
#[test]
fn merged_pr_delivers_without_closure_record() {
    let mut db = Database::start();
    let (repo, mut c, repository, _) = fixture_with_settings(&mut db, "t839_merged_pr");
    crate::adapters::github::set_fake_issue(&repo.root.path().join("fakes"), &serde_json::from_value(json!({"number":2761,"title":"Prerequisite PR","body":null,"state":"open","updated_at":"2026-11-03T23:00:00Z","pull_request":{"url":"https://api.github.com/repos/owner/sandbox/pulls/2761"}})).unwrap());
    let waiter = intake(&mut c, &repository, 42);
    ingest_brief_kind(&mut c, waiter, &[2761], "pull_request");
    reply(&mut c, waiter, Reply::AuditAgrees);
    c.observe(
        "t839-merged-control",
        Observation::Prerequisite(ExternalFact {
            updated_at: None,
            repository,
            kind: PrerequisiteKind::PullRequest,
            number: 2761,
            status: DependencyStatus::Delivered,
            reason: None,
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
    assert!(
        super::super::closures(&c.snapshot().unwrap(), waiter)
            .unwrap()
            .is_empty()
    );
}

/// #839 / Q27, 2026-11-05: closure is provisional until planning starts.
#[test]
fn reopened_root_before_planning_waits_then_closed_again_releases() {
    let mut db = Database::start();
    let (_repo, mut c, repository, _) = fixture_with_settings(&mut db, "t839_reopen");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761, 2772]);
    external(&mut c, &repository, 2761, DependencyStatus::Abandoned);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
    external(&mut c, &repository, 2761, DependencyStatus::Pending);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    external(&mut c, &repository, 2772, DependencyStatus::Abandoned);
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].waiting_prerequisite,
        Some(2761)
    );
    external(&mut c, &repository, 2761, DependencyStatus::Abandoned);
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
}

/// #839: controller open must read an upstream root again even after closure.
#[test]
fn open_reads_current_reopened_root_instead_of_replaying_closed_fact() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_open_current");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761, 2772]);
    external(&mut c, &repository, 2761, DependencyStatus::Abandoned);
    set_issue(&repo, 2761, "open", None);
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        c.prerequisite_evidence(waiter).unwrap()[0].state,
        PrerequisiteState::Open
    );
    set_issue(&repo, 2761, "closed", Some("not_planned"));
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
}

/// #839: catch-up observations replace an old closure with current open state.
#[test]
fn catch_up_reads_reopened_root_before_planning() {
    let mut db = Database::start();
    let (repo, mut c, repository, _) = fixture_with_settings(&mut db, "t839_catch_current");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761, 2772]);
    external(&mut c, &repository, 2761, DependencyStatus::Abandoned);
    set_issue(&repo, 2761, "open", None);
    c.observe(
        "t839-reopen-catch-up",
        Observation::PrerequisiteRead {
            node: waiter,
            repository,
            kind: PrerequisiteKind::Issue,
            number: 2761,
        },
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
}

/// #839: a queued pre-deploy closure must not undo the current-state refresh.
#[test]
fn open_supersedes_old_queued_closure_with_current_read() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_old_inbox");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    c.observe(
        "old-closure-before-deploy",
        Observation::Prerequisite(ExternalFact {
            updated_at: None,
            repository,
            kind: PrerequisiteKind::Issue,
            number: 2761,
            status: DependencyStatus::Abandoned,
            reason: Some("not_planned".into()),
        }),
    )
    .unwrap();
    set_issue(&repo, 2761, "open", None);
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
}

/// #839: delayed closure events cannot overwrite a newer reopen observation.
#[test]
fn older_closed_event_cannot_undo_a_newer_reopen() {
    let mut db = Database::start();
    let (_repo, mut c, repository, _) = fixture_with_settings(&mut db, "t839_old_event");
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761, 2772]);
    for (key, status, at) in [
        ("close", DependencyStatus::Abandoned, "2026-11-03T23:00:00Z"),
        ("reopen", DependencyStatus::Pending, "2026-11-05T23:35:00Z"),
        (
            "late-close",
            DependencyStatus::Abandoned,
            "2026-11-03T23:00:00Z",
        ),
    ] {
        c.observe(
            key,
            Observation::Prerequisite(ExternalFact {
                updated_at: Some(at.parse().unwrap()),
                repository: repository.clone(),
                kind: PrerequisiteKind::Issue,
                number: 2761,
                status,
                reason: Some("not_planned".into()),
            }),
        )
        .unwrap();
        assert!(c.drive_once().unwrap());
    }
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        c.prerequisite_evidence(waiter).unwrap()[0].state,
        PrerequisiteState::Open
    );
}

/// #839: authoritative ingest reads retain their version against delayed webhooks.
#[test]
fn older_closure_cannot_replace_current_open_ingest_read() {
    let mut db = Database::start();
    let (repo, mut c, repository, _) = fixture_with_settings(&mut db, "t839_ingest_version");
    crate::adapters::github::set_fake_issue(
        &repo.root.path().join("fakes"),
        &serde_json::from_value(json!({"number":2761,"title":"Prerequisite","body":null,
            "state":"open","updated_at":"2026-11-05T23:35:00Z"}))
        .unwrap(),
    );
    let waiter = intake(&mut c, &repository, 42);
    ingest(&mut c, waiter, &[2761]);
    c.observe(
        "late-closure-after-ingest",
        Observation::Prerequisite(ExternalFact {
            updated_at: Some("2026-11-03T23:00:00Z".parse().unwrap()),
            repository,
            kind: PrerequisiteKind::Issue,
            number: 2761,
            status: DependencyStatus::Abandoned,
            reason: Some("not_planned".into()),
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
}

/// #839: startup revokes delivery before a queued resume can admit planning.
#[test]
fn startup_revokes_closed_delivery_before_resume() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_resume_current");
    let waiter = intake(&mut c, &repository, 42);
    event(
        &mut c,
        Event::Command {
            node: waiter,
            id: 1,
            command: Command::Pause,
        },
    );
    ingest(&mut c, waiter, &[2761]);
    external(&mut c, &repository, 2761, DependencyStatus::Abandoned);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Delivered
    );
    assert!(!c.world().unwrap().nodes[waiter.0 as usize].planning_started);
    c.observe(
        "resume-before-open",
        Observation::Kernel(Event::Command {
            node: waiter,
            id: 2,
            command: Command::Resume,
        }),
    )
    .unwrap();
    set_issue(&repo, 2761, "open", None);
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Pending
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
}
