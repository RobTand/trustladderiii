use super::*;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;
use tl3_controller::{
    Ownership,
    adapters::flux_checks::{Adapter, Preparation, PreparedCheck},
    checks::CheckRun,
    cli::Input,
    executor::{Delivery, Executor, Output, Reconciliation},
    run_backend::{self as api, *},
};
use tl3_kernel::{EffectKind, NodeId};
use tl3_store::Store;

fn reservation() -> Reservation {
    Reservation {
        cpus: 1.try_into().unwrap(),
        gpus: 0,
        memory_bytes: 2147483648_u64.try_into().unwrap(),
        memory_guarantee: MemoryGuarantee::DeclaredOnly,
        parallelism: 1.try_into().unwrap(),
        native_threads: 1.try_into().unwrap(),
    }
}
fn placement() -> Placement {
    Placement {
        architecture: "x86_64".into(),
        host_dependency: Some(HostDependency {
            host: "dl380g10".into(),
            reason: "provisioned amd64 harness images".into(),
        }),
    }
}
fn delivery() -> Delivery {
    let mut d: Delivery = serde_json::from_value(json!({
        "key":"effect:0:5", "epoch":1, "output":{"Effect":{"node":0,"seq":5,"kind":"SubmitChecks","head":null}},
        "generation":4, "head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "repository":{"owner":"owner","name":"sandbox","installation":3},
        "issue":12,"paths":["value.py"],"attempt":1,"round":0,"job_generation":4
    })).unwrap();
    d.checks = Some(CheckRun {
        repository: "/served/repo".into(),
        base_sha: tl3_worktrees::CommitId::new("a".repeat(40)).unwrap(),
        reviewed_sha: None,
        reproducing_unavailable: None,
        plan: vec![],
        selected_failures: vec![],
    });
    d
}
struct Staging;
impl Preparation for Staging {
    fn prepare(&mut self, d: &Delivery) -> tl3_controller::Result<PreparedCheck> {
        let run = d.checks.clone().unwrap();
        let p = placement();
        let blob = BlobRef {
            path: "/immutable/preflight".into(),
            sha256: "0".repeat(64),
            bytes: 1,
        };
        let request = Request {
            pass_flux_uri: false,
            repo: "owner/sandbox".into(),
            tree: TreeIdentity {
                head: "c".repeat(40),
                tree: "d".repeat(40),
                input_digest: "0".repeat(64),
                root: "/immutable/input".into(),
                manifest: "/immutable/manifest".into(),
            },
            suite: Suite::DaytimeChecks {
                base: run.base_sha.as_str().into(),
                head: d.head_sha.clone().unwrap(),
                reviewed: None,
                plan_digest: durable::digest(&serde_json::to_vec(&run.plan).unwrap()),
            },
            reservation: reservation(),
            runner: RunnerIdentity {
                executable: "/immutable/collector".into(),
                executable_digest: "0".repeat(64),
                arguments: vec![],
                manifest: "/immutable/collector.json".into(),
                manifest_digest: "0".repeat(64),
            },
            environment: BTreeMap::new(),
            runtime: RuntimeIdentity {
                manifest: "/immutable/runtime.json".into(),
                manifest_digest: "0".repeat(64),
                runtime: ExecutionRuntime::Native {
                    interpreter: "/usr/bin/python3".into(),
                },
                architecture: p.architecture,
                host_dependency: p.host_dependency,
                expected_image: None,
                preflight: blob,
                image: None,
                cleanup: None,
            },
            scratch_root: "/var/tmp/flux-vanilla".into(),
            walltime_seconds: 60.try_into().unwrap(),
            no_progress_seconds: None,
        };
        Ok(PreparedCheck {
            request,
            run,
            changed_paths: vec!["value.py".into()],
            selected_tests: Some(0),
            always_tests: vec![],
        })
    }
    fn capacity(&self, _: &Delivery) -> tl3_controller::Result<(Reservation, Placement)> {
        Ok((reservation(), placement()))
    }
}
#[derive(Default)]
struct State {
    submits: usize,
    cancels: usize,
    unknown_submit: bool,
    binding: Option<RunBinding>,
    terminal: Option<Terminal>,
    probe: Option<Availability>,
    authority: Option<ProgressAuthority>,
    phase: Option<NativePhase>,
    emit_progress: bool,
}
struct Backend {
    state: Mutex<State>,
    changed: Condvar,
    attached: mpsc::Sender<Owner>,
    records: std::path::PathBuf,
}
impl Backend {
    fn terminal(&self, ending: Ending) {
        self.state.lock().unwrap().terminal = Some(Terminal {
            ending,
            reason: "trusted stopped evidence fixture".into(),
            receipt: BlobRef {
                path: "/output/receipt".into(),
                sha256: "1".repeat(64),
                bytes: 1,
            },
        });
        self.changed.notify_all();
    }
}
impl RunBackend for Backend {
    fn submit(&self, _: Request, intent: SubmissionIntent) -> api::Result<Submission> {
        // The external boundary observes the committed controller intent before accepting.
        let journal = self.records.join(format!("{}.job", intent.correlation));
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(journal).unwrap()).unwrap();
        assert_eq!(saved["intent"], json!(intent));
        assert!(saved["binding"].is_null());
        let mut state = self.state.lock().unwrap();
        state.submits += 1;
        state.terminal = None;
        let binding = RunBinding {
            id: BackendId {
                backend: BackendTag::Flux,
                instance: intent.instance.clone(),
                uri: intent.uri.clone(),
                job: canonical_job_id("f123").unwrap(),
            },
            intent,
        };
        state.binding = Some(binding.clone());
        Ok(if state.unknown_submit {
            Submission::Unknown {
                reason: "acceptance reply lost".into(),
            }
        } else {
            Submission::Accepted { binding }
        })
    }
    fn attach(
        &self,
        owner: &Owner,
        stop: &WatchStop,
        emit: &mut dyn FnMut(&Event) -> api::Result<()>,
    ) -> api::Result<api::Observation> {
        self.attached.send(owner.clone()).unwrap();
        let mut state = self.state.lock().unwrap();
        if state.emit_progress {
            let run = state.binding.as_ref().unwrap().id.clone();
            drop(state);
            let digest = "3".repeat(64);
            emit(&Event {
                sequence: 7,
                digest: digest.clone(),
                fact: NativeFact::Start,
                progress: NativeProgress {
                    owner: owner.clone(),
                    run,
                    cursor: EventCursor {
                        sequence: 7,
                        digest,
                    },
                    phase: NativePhase::Running,
                    authority: ProgressAuthority::Historical,
                },
            })?;
            state = self.state.lock().unwrap();
        }
        while state.terminal.is_none() && !stop.is_stopped() {
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
        Ok(match state.terminal.clone() {
            Some(terminal) => api::Observation::Terminal {
                binding: state.binding.clone(),
                terminal,
            },
            None => api::Observation::Unknown {
                binding: state.binding.clone(),
                reason: "observer stopped".into(),
            },
        })
    }
    fn cancel(&self, _: &Owner) -> api::Result<CancelAck> {
        let mut state = self.state.lock().unwrap();
        state.cancels += 1;
        Ok(CancelAck {
            acknowledged: true,
            binding: state.binding.clone(),
            terminal: None,
        })
    }
    fn probe(&self, r: &Reservation, p: &Placement) -> api::Result<Probe> {
        assert_eq!(r, &reservation());
        assert_eq!(p, &placement());
        Ok(Probe {
            availability: self
                .state
                .lock()
                .unwrap()
                .probe
                .clone()
                .unwrap_or(Availability::Busy),
            reason: "eligible dl380 census".into(),
            total_cpus: 40,
            free_cpus: 0,
            total_gpus: 0,
            free_gpus: 0,
            memory_guarantee: MemoryGuarantee::DeclaredOnly,
        })
    }
    fn liveness(&self, owner: &Owner) -> api::Result<Liveness> {
        let state = self.state.lock().unwrap();
        Ok(Liveness {
            owner: owner.clone(),
            run: state.binding.as_ref().map(|binding| binding.id.clone()),
            cursor: None,
            phase: if state.terminal.is_some() {
                NativePhase::Terminal
            } else {
                state.phase.unwrap_or(NativePhase::Running)
            },
            authority: if state.terminal.is_some() {
                ProgressAuthority::CommittedTerminal
            } else {
                state.authority.unwrap_or(ProgressAuthority::FreshNative)
            },
            observed_unix_ms: 1,
            terminal: state.terminal.clone(),
            evidence: Some(BlobRef {
                path: "/output/native-state".into(),
                sha256: "2".repeat(64),
                bytes: 1,
            }),
            reason: "bounded native fixture".into(),
        })
    }
    fn validate_allocation(&self, _: &Owner, _: &AllocationInput) -> api::Result<CpuGrant> {
        panic!("controller must not qualify allocation")
    }
    fn worker_payload(&self, _: &Owner, _: &Terminal) -> api::Result<WorkerPayload> {
        panic!("daytime adapter must not consume worker payload")
    }
    fn launch_context(&self, _: &Owner, _: &str) -> api::Result<LaunchContext> {
        panic!("controller must not launch")
    }
    fn begin_container_launch(
        &self,
        _: &Owner,
        _: &RuntimeOwnership,
    ) -> api::Result<ContainerLaunchIntent> {
        Err(BackendError::invalid(
            "controller fixture refuses container launch",
        ))
    }
    fn bind_container_launch(
        &self,
        _: &Owner,
        _: RuntimeOwnership,
    ) -> api::Result<ContainerLaunchBinding> {
        Err(BackendError::invalid(
            "controller fixture refuses container binding",
        ))
    }
    fn retain_runtime(&self, _: &Owner, _: RuntimeOwnership) -> api::Result<()> {
        panic!("controller must not own runtime launch")
    }
    fn cleanup_runtime(&self, _: &Owner) -> api::Result<CleanupResult> {
        panic!("controller must not infer cleanup")
    }
    fn daytime_payload(
        &self,
        _: &Owner,
        _: &Terminal,
    ) -> api::Result<tl3_controller::contracts::report::CheckReport> {
        Err(BackendError::invalid(
            "no semantic payload in infrastructure fixtures",
        ))
    }
}
fn setup(
    root: &Path,
    ownership: &mut Ownership,
) -> (Adapter, Arc<Backend>, mpsc::Receiver<Owner>, Options) {
    let options = production::flux_options(&super::flux_options::flux(root));
    let (entered, attached) = mpsc::channel();
    let backend = Arc::new(Backend {
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
        attached: entered,
        records: root.join("journal"),
    });
    let adapter = Adapter::new(
        backend.clone(),
        options.clone(),
        &backend.records,
        Staging,
        ownership,
    )
    .unwrap();
    (adapter, backend, attached, options)
}

/// FX / #850: a busy eligible host stays healthy; unknown and incompatible cannot clear an incident.
#[test]
fn flux_checks_probe_uses_submission_placement_and_keeps_busy_healthy() {
    let root = tempfile::tempdir().unwrap();
    let mut database = support::Database::start();
    let mut ownership =
        Ownership::claim(Store::open(&database.create("flux_probe")).unwrap()).unwrap();
    let (mut adapter, backend, _, _) = setup(root.path(), &mut ownership);
    let d = delivery();
    for (availability, healthy) in [
        (Availability::Busy, true),
        (Availability::Available, true),
        (Availability::Unknown, false),
        (Availability::Incompatible, false),
    ] {
        backend.state.lock().unwrap().probe = Some(availability);
        assert_eq!(adapter.probe(&d).unwrap(), healthy);
    }
    adapter.execute(&d).unwrap();
    assert_eq!(backend.state.lock().unwrap().submits, 1);
    assert!(!adapter.probe(&d).unwrap());
    drop((adapter, ownership));
    database.stop();
}

/// FX: acceptance ambiguity and a changed broker generation never authorize a second submission.
#[test]
fn flux_checks_restart_attaches_retained_intent_without_resubmission() {
    let root = tempfile::tempdir().unwrap();
    let mut database = support::Database::start();
    let mut ownership =
        Ownership::claim(Store::open(&database.create("flux_restart")).unwrap()).unwrap();
    let (mut adapter, backend, attached, mut options) = setup(root.path(), &mut ownership);
    backend.state.lock().unwrap().unknown_submit = true;
    let d = delivery();
    assert!(adapter.execute(&d).is_err());
    drop(adapter);
    options
        .environment
        .insert("FLUX_URI".into(), "local:///replacement".into());
    let mut adapter = Adapter::new(
        backend.clone(),
        options,
        &backend.records,
        Staging,
        &mut ownership,
    )
    .unwrap();
    let (sender, _inputs) = mpsc::channel();
    let source = adapter.source().unwrap().spawn(sender);
    let owner = attached.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(owner.correlation, durable::digest(d.key.as_bytes()));
    assert!(matches!(
        adapter.reconcile(&d).unwrap(),
        Reconciliation::Pending
    ));
    assert!(adapter.execute(&d).is_err());
    assert_eq!(backend.state.lock().unwrap().submits, 1);
    let mut changed = d.clone();
    changed.head_sha = Some("e".repeat(40));
    assert!(adapter.execute(&changed).is_err());
    let mut newer = d.clone();
    newer.epoch = 2;
    assert!(adapter.execute(&newer).is_err());
    assert!(
        adapter
            .execute(&d)
            .unwrap_err()
            .to_string()
            .contains("older epoch")
    );
    source.stop().unwrap();
    source.join().unwrap();
    assert_eq!(backend.state.lock().unwrap().cancels, 0);
    drop((adapter, ownership));
    database.stop();
}

/// FX: a cancel acknowledgement is insufficient; finish/cancel races retain the terminal outcome across restart.
#[test]
fn flux_checks_withdraw_waits_for_terminal_and_replays_durable_completion() {
    let root = tempfile::tempdir().unwrap();
    let mut database = support::Database::start();
    let mut ownership =
        Ownership::claim(Store::open(&database.create("flux_cancel")).unwrap()).unwrap();
    let (mut adapter, backend, attached, options) = setup(root.path(), &mut ownership);
    let d = delivery();
    adapter.execute(&d).unwrap();
    let (sender, inputs) = mpsc::channel();
    let running = adapter.source().unwrap().spawn(sender);
    attached.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut withdrawal = d.clone();
    withdrawal.key = "effect:0:6".into();
    withdrawal.output = Output::Effect {
        node: NodeId(0),
        seq: 6,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    assert!(adapter.execute(&withdrawal).is_err());
    assert!(matches!(
        adapter.reconcile(&withdrawal).unwrap(),
        Reconciliation::Pending
    ));
    assert!(
        inputs.try_recv().is_err(),
        "cancel acknowledgement became completion"
    );
    backend.terminal(Ending::Cancelled);
    let input = inputs.recv_timeout(Duration::from_secs(5)).unwrap();
    let Input::Completion { key, completion } = input else {
        panic!("expected completion")
    };
    assert_eq!(key, d.key);
    assert!(completion.observations.iter().any(|o| matches!(
        o,
        tl3_controller::executor::Observation::Checks {
            report: false,
            pass: false,
            ..
        }
    )));
    assert!(matches!(
        adapter.reconcile(&withdrawal).unwrap(),
        Reconciliation::Found(_)
    ));
    running.stop().unwrap();
    running.join().unwrap();
    drop(adapter);
    let mut restored = Adapter::new(
        backend.clone(),
        options,
        &backend.records,
        Staging,
        &mut ownership,
    )
    .unwrap();
    assert!(matches!(
        restored.reconcile(&d).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_eq!(backend.state.lock().unwrap().submits, 1);
    drop((restored, ownership));
    database.stop();
}

struct ScenarioWorkers(tl3_controller::adapters::workers::Fake);
fn scenario_completion(completion: &mut tl3_controller::executor::Completion) {
    use tl3_controller::executor::Observation;
    for observation in &mut completion.observations {
        match observation {
            Observation::Made { sha, .. } => *sha = "b".repeat(40),
            Observation::Plan { paths, .. } => *paths = vec!["value.py".into()],
            _ => {}
        }
    }
}
impl Executor for ScenarioWorkers {
    fn execute(
        &mut self,
        d: &Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        let mut completion = self.0.execute(d)?;
        scenario_completion(&mut completion);
        Ok(completion)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        let mut result = self.0.reconcile(d)?;
        if let Reconciliation::Found(completion) = &mut result {
            scenario_completion(completion);
        }
        Ok(result)
    }
}

fn drain(controller: &mut tl3_controller::Controller) {
    for _ in 0..200 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}

/// #850: the real controller closes the first service incident on an eligible busy census,
/// then submits a fresh check through the selected adapter. Unknown census retains the incident.
#[test]
fn flux_checks_controller_incident_clearance_allows_fresh_dispatch() {
    use tl3_controller::{
        Bootstrap, Controller, LabelledIssue, adapters,
        executor::{Observation, Upstream},
    };
    let root = tempfile::tempdir().unwrap();
    let mut database = support::Database::start();
    let mut ownership =
        Ownership::claim(Store::open(&database.create("flux_incident")).unwrap()).unwrap();
    let (mut adapter, backend, attached, _) = setup(root.path(), &mut ownership);
    let source = adapter.source().unwrap();
    let fakes = root.path().join("fakes");
    let mut registry = adapters::simulation(&fakes).unwrap();
    registry.register(Upstream::PrismaBuild, adapter);
    registry.register(
        Upstream::Workers,
        ScenarioWorkers(adapters::workers::Fake::new(&fakes)),
    );
    let mut c = Controller::open_owned(
        ownership,
        registry,
        Bootstrap {
            repos: vec![delivery().repository],
            operations: None,
            rob: 42,
            slots: 1,
            pool: 1,
            fail_fast: false,
        },
    )
    .unwrap();
    let mut allowances = tl3_config::Allowances::default();
    allowances.prismabuild.silence_secs = 1;
    allowances.prismabuild.no_progress_secs = 3;
    c.set_allowances(allowances);
    c.intake(LabelledIssue {
        delivery: "flux-incident-intake".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Flux incident recovery".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
    drain(&mut c);
    let (sender, inputs) = mpsc::channel();
    let running = source.spawn(sender);
    let mut plan = delivery().checks.unwrap();
    plan.plan = serde_json::from_value(
        json!([{"name":"regression", "kind":"regression", "tests":["test_value.py::test_value"]}]),
    )
    .unwrap();
    for round in 1..=2 {
        c.observe(
            &format!("flux-plan-{round}"),
            Observation::CheckPlan {
                node: NodeId(0),
                sha: "b".repeat(40),
                run: plan.clone(),
            },
        )
        .unwrap();
        drain(&mut c);
        attached.recv_timeout(Duration::from_secs(5)).unwrap();
        backend.terminal(Ending::InfraLost);
        let input = inputs.recv_timeout(Duration::from_secs(5)).unwrap();
        tl3_controller::ingress::apply(&mut c, input).unwrap();
        drain(&mut c);
    }
    let incident = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "incidents" && r.value["open"] == true && r.value["blocking"] == true)
        .unwrap();
    assert_eq!(incident.value["needs_rob"], false);
    assert!(
        incident.value["effect"].is_null(),
        "terminal job must leave a service incident"
    );
    assert!(c.world().unwrap().nodes[0].waiting_incident);
    backend.state.lock().unwrap().probe = Some(Availability::Unknown);
    std::thread::sleep(Duration::from_millis(1100));
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == incident.id && r.value["open"] == true)
    );
    let before = backend.state.lock().unwrap().submits;
    assert_eq!(before, 2);
    backend.state.lock().unwrap().probe = Some(Availability::Busy);
    std::thread::sleep(Duration::from_millis(1100));
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == incident.id && r.value["open"] == false)
    );
    assert!(!c.world().unwrap().nodes[0].waiting_incident);
    let after = backend.state.lock().unwrap().submits;
    assert_eq!(
        after,
        before + 1,
        "incident clearance must permit an automatic fresh dispatch"
    );
    running.stop().unwrap();
    running.join().unwrap();
    drop(c);
    database.stop();
}

/// FX / #369: historical progress is durable before ingress but only fresh native authority proves liveness.
#[test]
fn flux_checks_historical_progress_is_journaled_without_fabricating_liveness() {
    use tl3_controller::executor::Look;
    let root = tempfile::tempdir().unwrap();
    let mut database = support::Database::start();
    let mut ownership =
        Ownership::claim(Store::open(&database.create("flux_progress")).unwrap()).unwrap();
    let (mut adapter, backend, _attached, _) = setup(root.path(), &mut ownership);
    backend.state.lock().unwrap().emit_progress = true;
    let d = delivery();
    adapter.execute(&d).unwrap();
    let (sender, inputs) = mpsc::channel();
    let running = adapter.source().unwrap().spawn(sender);
    let input = inputs.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        input,
        Input::Observation {
            observation: tl3_controller::executor::Observation::Progress { .. },
            ..
        }
    ));
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(
            backend
                .records
                .join(format!("{}.job", durable::digest(d.key.as_bytes()))),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(journal["progress"]["cursor"]["sequence"], 7);
    assert!(matches!(
        inputs.recv_timeout(Duration::from_secs(5)).unwrap(),
        Input::Wake
    ));
    backend.state.lock().unwrap().authority = Some(ProgressAuthority::Historical);
    assert!(matches!(adapter.look(&d).unwrap(), Look::Pending));
    backend.state.lock().unwrap().authority = Some(ProgressAuthority::Unknown);
    assert!(matches!(adapter.look(&d).unwrap(), Look::Pending));
    backend.state.lock().unwrap().authority = Some(ProgressAuthority::FreshNative);
    backend.state.lock().unwrap().phase = Some(NativePhase::Queued);
    assert!(matches!(adapter.look(&d).unwrap(), Look::Alive { .. }));
    backend.state.lock().unwrap().phase = Some(NativePhase::Unknown);
    assert!(matches!(adapter.look(&d).unwrap(), Look::Pending));
    assert!(
        adapter
            .wait_record("foreign-owner", Path::new("/foreign-checkout"))
            .is_err()
    );
    running.stop().unwrap();
    running.join().unwrap();
    assert_eq!(backend.state.lock().unwrap().cancels, 0);
    drop((adapter, ownership));
    database.stop();
}
