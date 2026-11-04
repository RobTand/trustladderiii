//! #752 / Q27: mixed-release cancellation and generation fencing use the real controller.
use crate::support as database;

use super::*;
use serde_json::json;
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue,
    adapters::{self, workers::Supervisor},
    executor::{Delivery, Executor, Observation, Reconciliation, Upstream},
};
use tl3_kernel::{Command as KernelCommand, Event, NodeId};
use tl3_store::Store;

#[derive(Clone)]
struct Clock(Rc<Cell<u64>>);
impl tl3_controller::clock::Clock for Clock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}

fn open(config: &tl3_config::Config, root: &Path, clock: Clock) -> (Controller, Supervisor) {
    let supervisor = Supervisor::new(config, root, &crate::startup_ownership::ownership()).unwrap();
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Workers, supervisor.clone());
    let mut controller = Controller::open_with_clock(
        Store::open(&config.store).unwrap(),
        registry,
        Bootstrap::from_config(config).unwrap(),
        clock,
    )
    .unwrap();
    let mut allowances = config.allowances.clone();
    allowances.agent_worker.silence_secs = 1;
    allowances.agent_worker.no_progress_secs = 3;
    controller.set_allowances(allowances);
    (controller, supervisor)
}

fn drive(controller: &mut Controller) {
    while controller.drive_once().unwrap() {}
}

fn watch(controller: &mut Controller) -> Delivery {
    serde_json::from_value(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.table == "watches")
            .unwrap()
            .value
            .clone(),
    )
    .unwrap()
}

fn reply_documents(
    controller: &mut Controller,
) -> std::collections::BTreeMap<(String, String), Value> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| matches!(row.table.as_str(), "briefs" | "goals" | "worker_failures"))
        .map(|row| ((row.table, row.id), row.value))
        .collect()
}

#[test]
fn restarted_controller_cancels_legacy_worker_and_rejects_its_late_outcome() {
    let mut database = database::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
    config.store = database.create("supervisor_mixed_release");
    config.workers.runs = root.path().join("runs");
    config.workers.python = "/usr/bin/python3".into();
    config.models.key_file = root.path().join("model-key");
    fs::write(&config.models.key_file, "fixture-only").unwrap();
    config.models.base_url = "http://model.invalid/v1".parse().unwrap();
    config.hindsight.endpoint = "http://127.0.0.1:9".parse().unwrap();
    let entered = root.path().join("entered");
    config.workers.command = vec![
        "-c".into(),
        format!(
            "import pathlib,signal; print('{{\"kind\":\"early_end\",\"outcome\":\"not_actionable\",\"reason\":\"legacy outcome\"}}',flush=True); pathlib.Path({:?}).write_text('ready'); signal.pause()",
            entered.to_str().unwrap()
        ),
    ];
    let legacy = root.path().join("legacy.py");
    fs::write(&legacy, include_bytes!("legacy.py")).unwrap();
    let launcher = root.path().join("legacy-launcher.sh");
    // Stage the same per-attempt script the previous release used. $2 is the
    // new helper argument, deliberately replaced only for this legacy launch.
    fs::write(
        &launcher,
        "cp \"$1\" \"$3/supervisor.py\"\nexec /usr/bin/python3 \"$3/supervisor.py\" \"$3\"\n",
    )
    .unwrap();
    config.workers.launcher = vec![
        "/bin/sh".into(),
        launcher.to_string_lossy().into_owned(),
        legacy.to_string_lossy().into_owned(),
    ];
    let mut store = Store::open(&config.store).unwrap();
    store.put_worker_materials("0", &json!({
        "issue": {"number":42,"title":"Fix greeting","body":"Return hello.","author":"requester","comments":[]},
        "repo": {"readme":"Greeting library","file_list":["greeting.py"],"materials":[{"path":"greeting.py","content":"print('hello')\n"}]}
    })).unwrap();
    drop(store);
    let clock = Clock(Rc::new(Cell::new(1000)));
    let fakes = root.path().join("fakes");
    let (mut controller, _) = open(&config, &fakes, clock.clone());
    controller
        .intake(LabelledIssue {
            delivery: "mixed-release".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Fix greeting".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    drive(&mut controller);
    wait_for_file(root.path(), "entered");
    let original = watch(&mut controller);
    let attempt = fs::read_dir(&config.workers.runs)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("attempt-00000000"))
        .find(|attempt| attempt.join("supervisor.py").exists())
        .unwrap();
    let worker = check_identity(&attempt, "process.json");
    let pidfd = rustix::process::pidfd_open(
        rustix::process::Pid::from_raw(worker["pid"].as_i64().unwrap().try_into().unwrap())
            .unwrap(),
        rustix::process::PidfdFlags::empty(),
    )
    .unwrap();
    // Capture the result while it is in flight: normal cancellation retires
    // the attempt files before the delayed observation reaches the controller.
    let reply: Value =
        serde_json::from_slice(&fs::read(attempt.join("stdout.jsonl")).unwrap()).unwrap();
    let pinned_runtime = fs::read(attempt.join("launch.json")).unwrap();
    drop(controller);
    // Reopen the actual persisted controller with the current Rust launch path.
    config.workers.launcher.clear();
    let (mut controller, mut supervisor) = open(&config, &fakes, clock.clone());
    assert!(matches!(
        supervisor.reconcile(&original).unwrap(),
        Reconciliation::Pending
    ));
    controller.look_now().unwrap();
    assert_eq!(
        controller.world().unwrap().nodes[0].generation,
        original.generation
    );
    assert_eq!(
        fs::read(attempt.join("launch.json")).unwrap(),
        pinned_runtime
    );
    assert!(attempt.join("supervisor.py").exists());
    // The real liveness recovery dispatches StopRun, waits for termination,
    // then starts the next generation with the current Rust supervisor.
    clock.0.set(4000);
    controller.look_due().unwrap();
    drive(&mut controller);
    let newer = watch(&mut controller);
    assert!(newer.generation > original.generation);
    let mut exited = [rustix::event::PollFd::new(
        &pidfd,
        rustix::event::PollFlags::IN,
    )];
    assert_eq!(
        rustix::event::poll(&mut exited, Some(&rustix::event::Timespec::default())).unwrap(),
        1,
        "legacy worker must terminate before retry"
    );
    assert!(!matches!(
        supervisor.reconcile(&original).unwrap(),
        Reconciliation::Pending
    ));
    let before = serde_json::to_value(controller.world().unwrap()).unwrap();
    let documents = reply_documents(&mut controller);
    // Deliver the actual JSON produced by the old worker, delayed until after
    // cancellation and replacement. The live attempt never became a new run.
    controller
        .observe(
            "late-legacy-reply",
            Observation::WorkerReply {
                node: original.node(),
                generation: original.generation,
                role: "ingest".into(),
                reply,
                model: None,
            },
        )
        .unwrap();
    // observe enqueues ingress; dispatch it before checking either fence.
    drive(&mut controller);
    assert_eq!(
        reply_documents(&mut controller),
        documents,
        "an old reply must not replace the new generation's durable documents"
    );
    assert_eq!(
        serde_json::to_value(controller.world().unwrap()).unwrap(),
        before
    );
    assert_eq!(watch(&mut controller).generation, newer.generation);
    controller
        .observe(
            "cleanup-current-run",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: KernelCommand::Stop,
            }),
        )
        .unwrap();
    drive(&mut controller);
    assert!(controller.world().unwrap().nodes[0].phase.is_closed());
    drop(controller);
    database.stop();
}
