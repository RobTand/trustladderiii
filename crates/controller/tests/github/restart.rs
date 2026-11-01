use super::*;
use tl3_controller::executor::CommentKind;
use tl3_controller::steering::Task;
use tl3_kernel::{Command as KernelCommand, Event};

const RESTART: &str = "Restart this request.";
const EDITED_TITLE: &str = "Add a green button";
const EDITED_BODY: &str = "Show a green button instead.";

fn restart_comment(controller: &mut Controller, id: u64) {
    let mut comment = fixture(true)["comment"].clone();
    comment["id"] = json!(id);
    comment["body"] = json!(RESTART);
    controller
        .observe(
            &format!("restart-{id}"),
            Observation::GitHubComment {
                repository: bootstrap().repos.remove(0),
                issue: 12,
                comment: serde_json::from_value(comment).unwrap(),
            },
        )
        .unwrap();
}

fn classifier_state(controller: &mut Controller, id: u64) -> Value {
    let effect = controller
        .effect(&format!("classify:owner/sandbox/3/{id}"))
        .unwrap()
        .expect("the comment was classified");
    effect.payload["steering"]["Classify"]["inputs"]["state"].clone()
}

fn edit_issue(api: &Server) {
    let mut state = api.state.lock().unwrap();
    state.issue_title = Some(EDITED_TITLE.into());
    state.issue_body = Some(EDITED_BODY.into());
}

fn assert_restarted(controller: &mut Controller, api: &Server, node: &str, original: u64) {
    let snapshot = controller.snapshot().unwrap();
    let request = &snapshot
        .rows
        .iter()
        .find(|r| r.table == "requests" && r.id == node)
        .expect("restart filed a new request")
        .value;
    assert_eq!(request["issue"], 12, "the restart reuses the issue");
    assert_eq!(request["title"], EDITED_TITLE);
    assert_eq!(request["source"], EDITED_BODY);
    let link = &snapshot
        .rows
        .iter()
        .find(|r| r.table == "request_links" && r.id == node)
        .expect("the new request links to the old one")
        .value;
    assert_eq!(link["original"], original);
    assert_eq!(link["authorized_by"], 42);
    assert_eq!(link["issue"]["number"], 12);
    assert_eq!(link["issue"]["title"], EDITED_TITLE);
    assert_eq!(link["issue"]["body"], EDITED_BODY);
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|r| matches!(r.table.as_str(), "replacement" | "replacement_pending"))
    );
    let state = api.state.lock().unwrap();
    assert!(state.linked.is_empty(), "a restart files no new issue");
    assert!(!state.closed);
}

pub(super) fn open_request(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("hold-merges"), "hold").unwrap();
    let api = Server::new();
    let mut controller =
        steering::race_controller(settings, root.path(), &api, json!({"kind": "restart"}));
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    edit_issue(&api);
    restart_comment(&mut controller, 20001);
    drain(&mut controller);
    assert_eq!(
        classifier_state(&mut controller, 20001)["restartable"],
        true
    );
    let world = controller.world().unwrap();
    assert_eq!(world.nodes.len(), 2, "restart starts a new request");
    assert_eq!(world.nodes[0].phase, Phase::Closed(Outcome::Replaced));
    assert!(!world.nodes[1].phase.is_closed());
    assert_restarted(&mut controller, &api, "1", 0);
    println!("test restart_open_request_starts_from_edited_issue ... ok");
}

pub(super) fn stopped_request(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("hold-merges"), "hold").unwrap();
    let api = Server::new();
    let mut controller =
        steering::race_controller(settings, root.path(), &api, json!({"kind": "restart"}));
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    controller
        .observe(
            "stop-before-restart",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: KernelCommand::Stop,
            }),
        )
        .unwrap();
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Stopped)
    );
    {
        let state = api.state.lock().unwrap();
        assert!(!state.closed, "Q49: a stop leaves the issue open");
        assert!(state.comments.iter().any(|c| {
            c["body"]
                .as_str()
                .unwrap()
                .contains("Comment `restart` on this issue to start again.")
        }));
    }
    edit_issue(&api);
    restart_comment(&mut controller, 20002);
    drain(&mut controller);
    assert_eq!(
        classifier_state(&mut controller, 20002)["restartable"],
        true
    );
    let world = controller.world().unwrap();
    assert_eq!(world.nodes.len(), 2, "restart files a new request");
    assert_eq!(world.nodes[0].phase, Phase::Closed(Outcome::Stopped));
    assert!(!world.nodes[1].phase.is_closed());
    assert_restarted(&mut controller, &api, "1", 0);
    assert_eq!(
        api.state.lock().unwrap().reopens,
        0,
        "an open issue is not reopened"
    );
    println!("test restart_stopped_request_files_linked_request ... ok");
}

pub(super) fn merged_request(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let api = Server::new();
    let mut controller =
        steering::race_controller(settings, root.path(), &api, json!({"kind": "restart"}));
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    restart_comment(&mut controller, 20003);
    drain(&mut controller);
    assert_eq!(
        classifier_state(&mut controller, 20003)["restartable"],
        false
    );
    assert_eq!(
        controller.world().unwrap().nodes.len(),
        1,
        "restart is not offered on merged work"
    );
    println!("test restart_not_offered_on_merged_request ... ok");
}

fn restart_delivery() -> Delivery {
    let mut restart = delivery(Notice::FileReplacement);
    restart.key = "restart:owner/sandbox/3/20004".into();
    restart.output = Out::Comment {
        node: NodeId(0),
        kind: CommentKind::Restart,
    };
    restart.steering = Some(Task::Restart {
        comment_key: "owner/sandbox/3/20004".into(),
        text: RESTART.into(),
        issue: None,
    });
    restart
}

pub(super) fn reopens_issue_tl3_closed() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapter(&api.uri(), root.path());
    edit_issue(&api);
    let event = |id: u64, event: &str, login: &str| {
        json!({"id": id, "event": event, "actor": {"id": 7, "login": login},
            "created_at": format!("2026-10-30T0{id}:00:00Z")})
    };
    // Only the latest close counts: a person closing it again keeps it closed.
    let history = vec![
        event(1, "closed", "factory[bot]"),
        event(2, "reopened", "owner"),
        event(3, "closed", "owner"),
    ];
    for (closer, events, reopened) in [
        ("factory[bot]", vec![], true),
        ("owner", vec![], false),
        ("owner after TL3", history, false),
    ] {
        {
            let mut state = api.state.lock().unwrap();
            state.closed = true;
            state.closed_by = Some(closer.into());
            state.issue_events = events;
            state.reopens = 0;
        }
        let completion = adapter.execute(&restart_delivery()).unwrap();
        let [
            Observation::LinkedRequest {
                issue, original, ..
            },
        ] = completion.observations.as_slice()
        else {
            panic!("restart did not report the issue: {completion:?}");
        };
        assert_eq!(*original, NodeId(0));
        assert_eq!(issue.number, 12);
        assert_eq!(issue.title, EDITED_TITLE);
        assert_eq!(issue.body.as_deref(), Some(EDITED_BODY));
        let state = api.state.lock().unwrap();
        assert_eq!(state.reopens, usize::from(reopened), "closed by {closer}");
        assert_eq!(state.closed, !reopened, "closed by {closer}");
        assert_eq!(issue.state, if reopened { "open" } else { "closed" });
    }
    println!("test restart_reopens_only_an_issue_tl3_closed ... ok");
}

struct RestartWorkers {
    fake: adapters::workers::Fake,
    early: Option<tl3_kernel::EarlyEnd>,
}
impl Executor for RestartWorkers {
    fn execute(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        use tl3_controller::executor::Completion;
        if delivery.is_classifier() {
            return Ok(Completion::confirmed(vec![Observation::Classified {
                key: delivery.key.clone(),
                reply: Some(json!({"kind": "restart"})),
            }]));
        }
        if let Some(end) = self.early
            && delivery.node() == NodeId(0)
            && matches!(
                delivery.output,
                Out::Effect {
                    kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Ingest),
                    ..
                }
            )
        {
            return Ok(Completion::confirmed(vec![Observation::Kernel(
                Event::Reply {
                    node: delivery.node(),
                    generation: delivery.generation,
                    reply: tl3_kernel::Reply::IngestEarlyEnd(end),
                },
            )]));
        }
        self.fake.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}

/// Production always has an operations issue; failed restarts report there.
fn operations_controller(
    settings: &tl3_config::Store,
    root: &Path,
    api: &Server,
    early: Option<tl3_kernel::EarlyEnd>,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::GitHub, adapter(&api.uri(), root));
    registry.register(
        Upstream::Workers,
        RestartWorkers {
            fake: adapters::workers::Fake::new(root),
            early,
        },
    );
    let mut boot = bootstrap();
    boot.operations = Some(tl3_config::OperationsIssue {
        owner: "owner".into(),
        name: "tl3".into(),
        installation_id: 3,
        number: 17,
    });
    Controller::open(Store::open(settings).unwrap(), registry, boot).unwrap()
}

fn step_until(controller: &mut Controller, mut done: impl FnMut(&mut Controller) -> bool) {
    for _ in 0..200 {
        if done(controller) {
            return;
        }
        assert!(controller.drive_once().unwrap(), "controller settled first");
    }
    panic!("condition not reached");
}

fn stop(controller: &mut Controller) {
    controller
        .observe(
            "stop-before-restart",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: KernelCommand::Stop,
            }),
        )
        .unwrap();
}

fn open_restart_incident(controller: &mut Controller) -> Option<Value> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["effect"]
                    .as_str()
                    .is_some_and(|key| key.starts_with("restart:"))
        })
        .map(|r| r.value)
}

pub(super) fn stopping_request(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("hold-merges"), "hold").unwrap();
    let api = Server::new();
    let mut controller = operations_controller(settings, root.path(), &api, None);
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    stop(&mut controller);
    step_until(&mut controller, |c| {
        c.world().unwrap().nodes[0].phase == Phase::Stopping(Outcome::Stopped)
    });
    edit_issue(&api);
    restart_comment(&mut controller, 20005);
    step_until(&mut controller, |c| {
        c.snapshot().unwrap().rows.iter().any(|r| {
            r.table == "github_comments"
                && r.value["comment"]["id"] == 20005
                && r.value.get("classification").is_some()
        })
    });
    let state = classifier_state(&mut controller, 20005);
    assert_eq!(
        state["phase"], "Stopping(Stopped)",
        "restart arrived mid-stop"
    );
    assert_eq!(state["restartable"], true);
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    drain(&mut controller);
    let world = controller.world().unwrap();
    assert_eq!(
        world.nodes.len(),
        2,
        "the restart starts once the stop closes"
    );
    assert_eq!(world.nodes[0].phase, Phase::Closed(Outcome::Stopped));
    assert!(!world.nodes[1].phase.is_closed());
    assert_restarted(&mut controller, &api, "1", 0);
    println!("test restart_during_stopping_starts_after_close ... ok");
}

pub(super) fn unreadable_issue(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("hold-merges"), "hold").unwrap();
    let api = Server::new();
    let mut controller = operations_controller(settings, root.path(), &api, None);
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    restart_comment(&mut controller, 20006);
    step_until(&mut controller, |c| {
        let pending = c.snapshot().unwrap().pending_effects;
        pending.iter().any(|e| e.key.starts_with("restart:"))
            && !pending.iter().any(|e| e.key.starts_with("notice:"))
    });
    api.state.lock().unwrap().issue_read_status = Some(403);
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Replaced)
    );
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "replacement" && r.id == "0"),
        "the restart stays pending"
    );
    let incident = open_restart_incident(&mut controller).expect("Rob is told");
    // Clearing the incident retries the read, so a Replaced request is no dead end.
    api.state.lock().unwrap().issue_read_status = None;
    edit_issue(&api);
    let mut comment = fixture(true)["comment"].clone();
    comment["id"] = json!(20016);
    comment["body"] = json!(format!(
        "clear incident {}",
        incident["id"].as_str().unwrap()
    ));
    controller
        .observe(
            "clear-restart-incident",
            Observation::GitHubComment {
                repository: bootstrap().repos.remove(0),
                issue: 12,
                comment: serde_json::from_value(comment).unwrap(),
            },
        )
        .unwrap();
    drain(&mut controller);
    let world = controller.world().unwrap();
    assert_eq!(world.nodes.len(), 2, "clearing the incident restarts");
    assert!(!world.nodes[1].phase.is_closed());
    assert_restarted(&mut controller, &api, "1", 0);
    assert!(open_restart_incident(&mut controller).is_none());
    println!("test restart_unreadable_issue_retries_after_incident_clears ... ok");
}

pub(super) fn gone_issue(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("hold-merges"), "hold").unwrap();
    let api = Server::new();
    let mut controller = operations_controller(settings, root.path(), &api, None);
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    stop(&mut controller);
    drain(&mut controller);
    api.state.lock().unwrap().intake_target_status = Some(410);
    restart_comment(&mut controller, 20007);
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes.len(),
        1,
        "nothing is filed"
    );
    let snapshot = controller.snapshot().unwrap();
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|r| matches!(r.table.as_str(), "replacement" | "replacement_pending")),
        "the pending restart is closed"
    );
    assert!(open_restart_incident(&mut controller).is_none());
    let state = api.state.lock().unwrap();
    assert!(state.linked.is_empty());
    assert!(state.comments.iter().any(|c| {
        c["body"]
            .as_str()
            .unwrap()
            .contains("could not be found, so the restart was dropped")
    }));
    println!("test restart_gone_issue_drops_with_a_note ... ok");
}

pub(super) fn duplicate_request(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    // Keep the restarted request open, so its merge doesn't close the issue again.
    fs::write(root.path().join("hold-merges"), "hold").unwrap();
    let api = Server::new();
    api.state.lock().unwrap().closed_by = Some("factory[bot]".into());
    let mut controller = operations_controller(
        settings,
        root.path(),
        &api,
        Some(tl3_kernel::EarlyEnd::Duplicate),
    );
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Duplicate)
    );
    assert!(api.state.lock().unwrap().closed, "TL3 closed the duplicate");
    edit_issue(&api);
    restart_comment(&mut controller, 20008);
    drain(&mut controller);
    assert_eq!(
        classifier_state(&mut controller, 20008)["restartable"],
        true
    );
    let world = controller.world().unwrap();
    assert_eq!(world.nodes.len(), 2);
    assert_eq!(world.nodes[0].phase, Phase::Closed(Outcome::Duplicate));
    assert!(!world.nodes[1].phase.is_closed());
    assert_restarted(&mut controller, &api, "1", 0);
    assert_eq!(
        api.state.lock().unwrap().reopens,
        1,
        "TL3 reopens its own close"
    );
    println!("test restart_duplicate_request_reopens_issue ... ok");
}
