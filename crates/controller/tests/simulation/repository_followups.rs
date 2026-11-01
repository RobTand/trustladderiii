use super::*;

fn open_at(settings: &tl3_config::Store, root: &Path, boot: Bootstrap) -> Controller {
    Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root).unwrap(),
        boot,
    )
    .unwrap()
}

fn close(mut controller: Controller, settings: &tl3_config::Store) -> Snapshot {
    let snapshot = controller.snapshot().unwrap();
    drop(controller);
    controller_lock_barrier(settings);
    snapshot
}

pub(super) fn corrupt_index(settings: &tl3_config::Store, corruption: &str) {
    let root = tempfile::tempdir().unwrap();
    let before = close(open(settings, root.path()), settings);
    let mut commit = Commit::new("corrupt-repository-index", json!(corruption), &before);
    let expected = match corruption {
        "count" => {
            commit.rows.push(RowChange::Put {
                table: "gates".into(),
                id: "1".into(),
                value: before
                    .rows
                    .iter()
                    .find(|row| row.table == "gates")
                    .unwrap()
                    .value
                    .clone(),
            });
            "stored repositories and gates differ"
        }
        "invalid" | "duplicate" | "gap" => {
            if corruption != "duplicate" {
                commit.rows.push(RowChange::Delete {
                    table: "repos".into(),
                    id: "0".into(),
                });
            }
            let (id, error) = match corruption {
                "invalid" => ("invalid", "invalid repository index"),
                "duplicate" => ("00", "duplicate repository index"),
                "gap" => ("1", "gap in repository indices"),
                _ => unreachable!(),
            };
            commit.rows.push(RowChange::Put {
                table: "repos".into(),
                id: id.into(),
                value: json!(bootstrap().repos[0]),
            });
            error
        }
        _ => unreachable!(),
    };
    let mut store = Store::open(settings).unwrap();
    store.commit(&commit).unwrap();
    let corrupted = store.load().unwrap();
    // Include a capacity change and addition so refusal must precede all reconciliation writes.
    let mut boot = bootstrap();
    boot.slots = 3;
    boot.repos.push(Repository {
        owner: "RobTand".into(),
        name: "prismabuild".into(),
        installation: 2,
    });
    let result = Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        boot,
    );
    let Err(error) = result else {
        panic!("{corruption} corruption must refuse startup");
    };
    assert!(
        matches!(error, tl3_controller::Error::Invalid(message) if message == expected),
        "expected {expected:?}, got {error}"
    );
    controller_lock_barrier(settings);
    let after = store.load().unwrap();
    assert_eq!(
        after.revision, corrupted.revision,
        "refusal must write nothing"
    );
    assert_eq!(
        after, corrupted,
        "refusal must preserve the entire snapshot"
    );
}

pub(super) fn comment_case_identity(settings: &tl3_config::Store, pull_request: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(settings, root.path());
    assert!(controller.intake(labelled()).unwrap());
    assert!(!controller.world().unwrap().nodes[0].phase.is_closed());
    let before = close(controller, settings);
    if pull_request {
        let mut commit = Commit::new("fixture-pull-request", json!({}), &before);
        commit.rows.push(RowChange::Put {
            table: "pull_requests".into(),
            id: "0".into(),
            value: json!({"number": 99}),
        });
        Store::open(settings).unwrap().commit(&commit).unwrap();
    }
    let mut boot = bootstrap();
    boot.repos[0].owner = "robtand".into();
    boot.repos[0].name = "TL3-SANDBOX".into();
    let repository = boot.repos[0].clone();
    let mut controller = open_at(settings, root.path(), boot);
    controller
        .observe(
            "case-changed-comment",
            Observation::GitHubComment {
                repository,
                issue: if pull_request { 99 } else { labelled().number },
                comment: tl3_github::Comment {
                    id: 55,
                    body: "Please update the greeting".into(),
                    user: tl3_github::Actor {
                        id: bootstrap().rob,
                        login: "rob".into(),
                    },
                    created_at: chrono::DateTime::UNIX_EPOCH,
                    updated_at: chrono::DateTime::UNIX_EPOCH,
                    issue_url: format!(
                        "https://api.github.com/repos/robtand/TL3-SANDBOX/issues/{}",
                        if pull_request { 99 } else { labelled().number }
                    ),
                },
            },
        )
        .unwrap();
    assert!(controller.drive_once().unwrap());
    assert!(controller.drive_once().unwrap());
    let snapshot = controller.snapshot().unwrap();
    let classifier = snapshot
        .pending_effects
        .iter()
        .find(|effect| effect.key.starts_with("classify:"))
        .expect("case-only config changes must still bind comments to the in-flight request");
    let delivery: Delivery = serde_json::from_value(classifier.payload.clone()).unwrap();
    assert_eq!(delivery.node(), NodeId(0));
    assert_eq!(delivery.issue, labelled().number);
    controller
        .observe(
            "case-changed-classified",
            Observation::Classified {
                key: classifier.key.clone(),
                reply: Some(json!({"kind": "unsure", "question": "Which greeting should change?"})),
            },
        )
        .unwrap();
    assert!(controller.drive_once().unwrap());
    let after = controller.snapshot().unwrap();
    let comment = after
        .rows
        .iter()
        .find(|row| row.table == "github_comments")
        .unwrap();
    assert_eq!(comment.value["node"], 0);
    assert_eq!(comment.value["classification"]["kind"], "unsure");
    assert_ne!(comment.value["ignored"], true);
    assert!(
        after
            .pending_effects
            .iter()
            .any(|effect| effect.key.starts_with("clarify:"))
    );
}

pub(super) fn capacity_and_addition(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(settings, root.path());
    assert!(controller.intake(labelled()).unwrap());
    let before = close(controller, settings);
    let mut boot = bootstrap();
    boot.slots = 3;
    let added = Repository {
        owner: "RobTand".into(),
        name: "prismabuild".into(),
        installation: 2,
    };
    boot.repos.insert(0, added.clone());
    let mut controller = open_at(settings, root.path(), boot.clone());
    let world = controller.world().unwrap();
    assert_eq!(
        world.gates.iter().map(|gate| gate.w).collect::<Vec<_>>(),
        vec![3, 3]
    );
    assert!(world.valid() && world.is_settled());
    assert_eq!(world.nodes[0].repo, tl3_kernel::RepoId(0));
    let after = controller.snapshot().unwrap();
    assert_eq!(after.revision.get(), before.revision.get() + 1);
    for table in ["repos", "gates"] {
        let rows: Vec<_> = after.rows.iter().filter(|row| row.table == table).collect();
        assert_eq!(rows.len(), 2);
    }
    let original = after
        .rows
        .iter()
        .find(|row| row.table == "repos" && row.id == "0")
        .unwrap();
    assert_eq!(original.value, json!(bootstrap().repos[0]));
    let appended = after
        .rows
        .iter()
        .find(|row| row.table == "repos" && row.id == "1")
        .unwrap();
    assert_eq!(appended.value, json!(added));
    for table in ["nodes", "requests"] {
        assert_eq!(
            after
                .rows
                .iter()
                .filter(|row| row.table == table)
                .collect::<Vec<_>>(),
            before
                .rows
                .iter()
                .filter(|row| row.table == table)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(after.pending_effects, before.pending_effects);
    assert_eq!(after.credit, before.credit);
    close(controller, settings);
    let mut controller = open_at(settings, root.path(), boot);
    assert_eq!(
        controller.snapshot().unwrap(),
        after,
        "second open must write nothing"
    );
    let mut issue = labelled();
    issue.owner = added.owner;
    issue.repo = added.name;
    issue.installation = added.installation;
    issue.delivery = "added-with-capacity".into();
    assert!(controller.intake(issue).unwrap());
    let world = controller.world().unwrap();
    assert_eq!(world.nodes[1].repo, tl3_kernel::RepoId(1));
    assert!(world.valid());
}
