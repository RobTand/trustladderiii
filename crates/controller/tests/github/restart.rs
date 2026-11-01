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
    for (closer, reopened) in [("factory[bot]", true), ("owner", false)] {
        {
            let mut state = api.state.lock().unwrap();
            state.closed = true;
            state.closed_by = Some(closer.into());
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
