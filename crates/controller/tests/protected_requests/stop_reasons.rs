use super::*;
use serde_json::Value;
use tl3_kernel::{Command, EarlyEnd, Event, Fact, NodeId, Reply};

struct Held;
impl Executor for Held {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
}

fn open(settings: &tl3_config::Store, root: &std::path::Path, ingest: bool) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Delivery, Held);
    if ingest {
        registry.register(Upstream::Workers, Held);
    }
    Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            operations: None,
            repos: vec![tl3_controller::Repository {
                owner: "owner".into(),
                name: "repo".into(),
                installation: 1,
            }],
            rob: 42,
            slots: 1,
            pool: 1,
            fail_fast: false,
        },
    )
    .unwrap()
}

fn start(c: &mut Controller) {
    c.intake(LabelledIssue {
        delivery: "close-reasons".into(),
        owner: "owner".into(),
        repo: "repo".into(),
        installation: 1,
        number: 42,
        title: "Implement greeting".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
    drain(c);
}

fn drain(c: &mut Controller) {
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("close-reason fixture did not settle");
}

fn send(c: &mut Controller, key: &str, event: Event) {
    let observation = match event {
        Event::Command { node, command, .. } => Observation::Steering { node, command },
        _ => Observation::Kernel(event),
    };
    c.observe(key, observation).unwrap();
    assert!(c.drive_once().unwrap());
}

fn reason(c: &mut Controller) -> Option<Value> {
    c.snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.table == "close_reasons" && row.id == "0")
        .map(|row| row.value)
}

fn stop(id: u64) -> Event {
    Event::Command {
        node: NodeId(0),
        id,
        command: Command::Stop,
    }
}

#[test]
fn user_stop_and_pr_close_keep_distinct_reasons_and_summary_evidence() {
    let mut db = support::Database::start();
    for (index, event, expected, wording) in [
        (0, stop(1), "user_stop", "you asked to stop"),
        (
            1,
            Event::Fact {
                node: NodeId(0),
                fact: Fact::PrClosedByPerson,
            },
            "pr_closed",
            "someone closed the pull request",
        ),
    ] {
        let settings = db.create(&format!("stop_cause_{index}"));
        let root = tempfile::tempdir().unwrap();
        let mut c = open(&settings, root.path(), false);
        start(&mut c);
        assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Queued);
        send(&mut c, "terminal-input", event);
        assert_eq!(reason(&mut c).unwrap()["reason"], expected);
        assert_eq!(
            c.world().unwrap().nodes[0].phase,
            Phase::Stopping(Outcome::Stopped)
        );
        let original = reason(&mut c);
        send(&mut c, "stop-again", stop(2));
        assert_eq!(
            reason(&mut c),
            original,
            "Stopping must not author a new reason"
        );
        drain(&mut c);
        assert_eq!(
            c.world().unwrap().nodes[0].phase,
            Phase::Closed(Outcome::Stopped)
        );
        let mut store = Store::open(&settings).unwrap();
        let summary = store
            .effects()
            .unwrap()
            .into_iter()
            .map(|effect| serde_json::from_value::<Delivery>(effect.payload).unwrap())
            .find(|delivery| {
                matches!(
                    delivery.output,
                    Output::Notice {
                        notice: Notice::Summary(_),
                        ..
                    }
                )
            })
            .expect("durable summary effect");
        assert_eq!(
            summary.evidence.as_ref().unwrap().stop_reason.as_deref(),
            Some(expected)
        );
        assert!(
            tl3_controller::evidence::summary(&summary)
                .unwrap()
                .contains(wording)
        );
        drop(c);
        let mut reopened = open(&settings, root.path(), false);
        assert_eq!(
            reason(&mut reopened),
            original,
            "reopen preserves historical reason"
        );
    }
    db.stop();
}

#[test]
fn stale_pr_close_and_out_of_order_stop_do_not_author_reasons() {
    let mut db = support::Database::start();
    let settings = db.create("stale_stop_cause");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path(), true);
    start(&mut c);
    assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Ingesting);
    send(
        &mut c,
        "pr-never-opened",
        Event::Fact {
            node: NodeId(0),
            fact: Fact::PrClosedByPerson,
        },
    );
    send(
        &mut c,
        "new-command",
        Event::Command {
            node: NodeId(0),
            id: 3,
            command: Command::Resume,
        },
    );
    c.observe("old-stop", Observation::Kernel(stop(2))).unwrap();
    assert!(c.drive_once().unwrap());
    assert!(reason(&mut c).is_none());
    assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Ingesting);
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents")
    );
    db.stop();
}

#[test]
fn accepted_early_end_projects_reason_but_stale_reply_does_not() {
    let mut db = support::Database::start();
    for (index, end, expected) in [
        (0, EarlyEnd::Duplicate, "duplicate"),
        (1, EarlyEnd::AlreadyDone, "already_done"),
        (2, EarlyEnd::NotActionable, "not_actionable"),
    ] {
        let settings = db.create(&format!("early_end_cause_{index}"));
        let root = tempfile::tempdir().unwrap();
        let mut c = open(&settings, root.path(), true);
        start(&mut c);
        let generation = c.world().unwrap().nodes[0].generation;
        send(
            &mut c,
            "stale-early-end",
            Event::Reply {
                node: NodeId(0),
                generation: generation + 1,
                reply: Reply::IngestEarlyEnd(end),
            },
        );
        assert!(reason(&mut c).is_none());
        send(
            &mut c,
            "current-early-end",
            Event::Reply {
                node: NodeId(0),
                generation,
                reply: Reply::IngestEarlyEnd(end),
            },
        );
        assert_eq!(
            reason(&mut c).expect("accepted stop has a durable reason")["reason"],
            expected
        );
    }
    db.stop();
}
