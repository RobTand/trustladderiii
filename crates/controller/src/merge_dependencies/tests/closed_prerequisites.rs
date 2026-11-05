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

/// #839 / Q59: the two live chain shapes retain their terminal upstream cause.
#[test]
fn live_closed_roots_are_visible_through_nine_and_four_waiter_chains() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_chains");
    let mut waiters = Vec::new();
    for (root, count) in [(2763, 9), (2773, 4), (2779, 1), (2781, 1)] {
        set_issue(&repo, root, "open", None);
        let root_node = intake(&mut c, &repository, root);
        for number in root + 1..=root + count {
            set_issue(&repo, number, "open", None);
            let node = intake(&mut c, &repository, number);
            ingest(&mut c, node, &[number - 1]);
            waiters.push((node, root, number));
        }
        reply(
            &mut c,
            root_node,
            Reply::IngestEarlyEnd(EarlyEnd::NotActionable),
        );
        reply(&mut c, root_node, Reply::AuditAgrees);
        confirm(&mut c, root_node);
        set_issue(&repo, root, "closed", Some("not_planned"));
        external(&mut c, &repository, root, DependencyStatus::Abandoned);
    }
    let model = status(&settings);
    for (node, root, number) in waiters {
        assert_eq!(
            c.world().unwrap().nodes[node.0 as usize].phase,
            Phase::Unplanned
        );
        let card = model
            .requests
            .iter()
            .find(|r| r.issue.contains(&format!("#{number}:")))
            .unwrap();
        assert!(
            card.waiting_on.contains(&format!(
                "#{root} to merge: closed without merge (not planned)"
            )),
            "{}",
            card.waiting_on
        );
        assert!(
            c.prerequisite_evidence(node)
                .unwrap()
                .iter()
                .any(|e| e.target.number == root
                    && e.state
                        == PrerequisiteState::ClosedUnmerged {
                            reason: ClosedPrerequisiteReason::NotPlanned
                        })
        );
    }
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
        DependencyStatus::Abandoned
    );
    set_issue(&repo, 2761, "open", None);
    let mut c = reopen(&repo, c, &settings);
    assert_eq!(
        c.world().unwrap().dependencies()[0].status,
        DependencyStatus::Abandoned
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
            repository: repository.clone(),
            kind: PrerequisiteKind::Issue,
            number: 2761,
            status: DependencyStatus::Abandoned,
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    let model = status(&settings);
    let expected = "waiting for #2761 to merge: closed without merge (not planned)";
    assert!(crate::status::render_text(&model).contains(expected));
    assert!(crate::status::render_html(&model).contains(expected));
}

/// #839 / Q27: closed PR evidence does not invent an abandonment transition.
#[test]
fn closed_pr_is_typed_and_displayed_without_changing_the_kernel_wait() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "t839_pr");
    crate::adapters::github::set_fake_issue(
        &repo.root.path().join("fakes"),
        &serde_json::from_value(json!({"number":2761,"title":"Prerequisite PR","body":null,
            "state":"open","updated_at":"2026-11-03T23:00:00Z","pull_request":{}}))
        .unwrap(),
    );
    let waiter = intake(&mut c, &repository, 42);
    ingest_brief_kind(&mut c, waiter, &[2761], "pull_request");
    reply(&mut c, waiter, Reply::AuditAgrees);
    c.observe(
        "t839-pr-closed",
        Observation::Prerequisite(ExternalFact {
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
        DependencyStatus::Pending
    );
    assert_eq!(
        c.world().unwrap().nodes[waiter.0 as usize].phase,
        Phase::Unplanned
    );
    assert_eq!(
        c.prerequisite_evidence(waiter).unwrap()[0].state,
        PrerequisiteState::ClosedUnmerged {
            reason: ClosedPrerequisiteReason::PullRequestClosed
        }
    );
    assert_eq!(
        status(&settings).requests[0].waiting_on,
        "waiting for #2761 to merge: closed without merge"
    );
    assert!(
        !c.snapshot()
            .unwrap()
            .pending_effects
            .iter()
            .any(|effect| effect.payload.to_string().contains("DependencyAbandoned"))
    );
    c.observe(
        "t839-pr-merged-later",
        Observation::Prerequisite(ExternalFact {
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
