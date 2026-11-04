mod support;

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
use support::{executor_probe::delivery, workers::Workers};
use tl3_controller::{
    Result,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, WaitRecord},
};
use tl3_kernel::{EffectKind, Role};
use tl3_store::{ExternalWait, Store};

type Calls = Arc<Mutex<Vec<(&'static str, &'static str, String)>>>;
struct Spy {
    name: &'static str,
    calls: Calls,
}
impl Spy {
    fn hit<T>(&self, method: &'static str, argument: String) -> Result<T> {
        self.calls
            .lock()
            .unwrap()
            .push((self.name, method, argument));
        Err(tl3_controller::Error::Invalid(self.name))
    }
}
impl Executor for Spy {
    fn input_incident(
        &mut self,
        notice: &tl3_controller::input_errors::IncidentNotice,
        epoch: i64,
    ) -> Result<()> {
        self.hit("input_incident", format!("{}:{epoch}", notice.key))
    }
    fn intake_question(
        &mut self,
        question: &tl3_controller::adoption::Clarification,
        epoch: i64,
    ) -> Result<()> {
        self.hit("intake_question", format!("{}:{epoch}", question.key()))
    }
    fn issue(
        &mut self,
        repository: &tl3_controller::Repository,
        number: u64,
    ) -> Result<tl3_github::Issue> {
        self.hit(
            "issue",
            format!("{}/{}#{number}", repository.owner, repository.name),
        )
    }
    fn app_login(&self, installation: u64) -> Option<String> {
        let _: Result<()> = self.hit("app_login", installation.to_string());
        Some(self.name.into())
    }
    fn prerequisite(
        &mut self,
        repository: &tl3_controller::Repository,
        kind: tl3_kernel::PrerequisiteKind,
        number: u64,
    ) -> Result<tl3_kernel::DependencyStatus> {
        self.hit(
            "prerequisite",
            format!("{}/{}#{number}:{kind:?}", repository.owner, repository.name),
        )
    }
    fn implementation(&self) -> &'static str {
        let _: Result<()> = self.hit("implementation", String::new());
        self.name
    }
    fn watch_wait(&mut self, delivery: &Delivery, _: Option<&ExternalWait>) -> Result<()> {
        self.hit("watch_wait", delivery.key.clone())
    }
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<WaitRecord> {
        self.hit("wait_record", format!("{key}:{}", checkout.display()))
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.hit("execute", delivery.key.clone())
    }
    fn observations(&mut self, delivery: &Delivery) -> Result<Vec<Observation>> {
        self.hit("observations", delivery.key.clone())
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.hit("reconcile", delivery.key.clone())
    }
    fn reconcile_cancellation(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.hit("reconcile_cancellation", delivery.key.clone())
    }
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        self.hit("look", delivery.key.clone())
    }
    fn probe(&mut self, delivery: &Delivery) -> Result<bool> {
        self.hit("probe", delivery.key.clone())
    }
    fn resume_after_quota(&mut self, delivery: &Delivery) -> Result<Option<Completion>> {
        self.hit("resume_after_quota", delivery.key.clone())
    }
    fn quota_resume_ms(&mut self, delivery: &Delivery, _: u64, _: u64) -> Result<u64> {
        self.hit("quota_resume_ms", delivery.key.clone())
    }
    fn stop_stalled(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.hit("stop_stalled", delivery.key.clone())
    }
    fn recover_deliveries(&mut self, after: u64, since_ms: u64) -> Result<Option<u64>> {
        self.hit("recover_deliveries", format!("{after}:{since_ms}"))
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> Result<()> {
        self.hit("retire_delivery", delivery.key.clone())
    }
    fn retire(&mut self, _: &mut Store) -> Result<()> {
        self.hit("retire", String::new())
    }
}

fn methods<'a>(source: &'a str, start: &str) -> BTreeSet<&'a str> {
    let body = source
        .split_once(start)
        .unwrap()
        .1
        .split_once("\n}\n")
        .unwrap()
        .0;
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .map(|signature| signature.split_once('(').unwrap().0)
        .collect()
}

#[test]
fn shared_router_and_probe_cover_the_executor_trait() {
    let expected = methods(include_str!("../src/executor.rs"), "pub trait Executor {");
    assert!(!expected.is_empty());
    assert_eq!(
        methods(
            include_str!("support/workers.rs"),
            "Executor for Workers<F> {"
        ),
        expected,
        "every Executor method must be forwarded, including new default methods"
    );
    assert_eq!(
        methods(
            include_str!("fixture_contracts.rs"),
            "impl Executor for Spy {"
        ),
        expected,
        "every Executor method must have a distinguishable forwarding probe"
    );
}

#[test]
fn workers_forward_to_the_selected_adapter_for_every_role() {
    let mut database = support::Database::start();
    let mut store = Store::open(&database.create("router_contract")).unwrap();
    for real_roles in [
        vec![],
        vec![Role::Make],
        vec![Role::Make, Role::Review],
        vec![Role::Review],
    ] {
        let calls = Calls::default();
        let real: Option<Box<dyn Executor>> = (!real_roles.is_empty()).then(|| {
            Box::new(Spy {
                name: "real",
                calls: calls.clone(),
            }) as Box<dyn Executor>
        });
        let primary = if real.is_some() { "real" } else { "fake" };
        let mut router = Workers::new(
            real,
            Spy {
                name: "fake",
                calls: calls.clone(),
            },
            real_roles.clone(),
            |_, _| panic!("an adapter error must propagate"),
        );
        for role in [
            Role::Ingest,
            Role::Audit,
            Role::Plan,
            Role::Make,
            Role::Review,
        ] {
            let d = delivery(role);
            let selected = if real_roles.contains(&role) {
                "real"
            } else {
                "fake"
            };
            assert_delivery_methods(&mut router, &d, selected, &calls);
        }
        let mut stop = delivery(Role::Make);
        stop.output = tl3_controller::executor::Output::Effect {
            node: stop.node(),
            seq: 2,
            kind: EffectKind::StopRun,
            head: None,
        };
        assert_delivery_methods(&mut router, &stop, primary, &calls);
        assert_eq!(router.implementation(), primary);
        assert_eq!(router.app_login(42).as_deref(), Some(primary));
        let question = tl3_controller::adoption::Clarification {
            repository: delivery(Role::Make).repository,
            number: 17,
        };
        assert!(router.intake_question(&question, 3).is_err());
        let incident = tl3_controller::input_errors::IncidentNotice {
            key: "incident:probe".into(),
            target: tl3_controller::input_errors::IssueTarget {
                repository: question.repository.clone(),
                issue: 17,
            },
            title: "A request".into(),
            summary: "Work needs help.".into(),
            id: "probe".into(),
        };
        assert!(router.input_incident(&incident, 3).is_err());
        assert!(router.wait_record("key", Path::new("checkout")).is_err());
        assert!(router.recover_deliveries(17, 29).is_err());
        assert!(router.issue(&question.repository, 17).is_err());
        assert!(
            router
                .prerequisite(
                    &question.repository,
                    tl3_kernel::PrerequisiteKind::Issue,
                    17
                )
                .is_err()
        );
        assert!(router.retire(&mut store).is_err());
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                (primary, "implementation", String::new()),
                (primary, "app_login", "42".into()),
                (primary, "intake_question", format!("{}:3", question.key())),
                (primary, "input_incident", "incident:probe:3".into()),
                (primary, "wait_record", "key:checkout".into()),
                (primary, "recover_deliveries", "17:29".into()),
                (
                    primary,
                    "issue",
                    format!(
                        "{}/{}#17",
                        question.repository.owner, question.repository.name
                    ),
                ),
                (
                    primary,
                    "prerequisite",
                    format!(
                        "{}/{}#17:Issue",
                        question.repository.owner, question.repository.name
                    )
                ),
                (primary, "retire", String::new()),
            ]
        );
    }
    drop(store);
    database.stop();
}

fn assert_delivery_methods(
    router: &mut dyn Executor,
    d: &Delivery,
    selected: &'static str,
    calls: &Calls,
) {
    calls.lock().unwrap().clear();
    assert!(router.execute(d).is_err());
    assert!(router.observations(d).is_err());
    assert!(router.reconcile(d).is_err());
    assert!(router.reconcile_cancellation(d).is_err());
    assert!(router.look(d).is_err());
    assert!(router.probe(d).is_err());
    assert!(router.resume_after_quota(d).is_err());
    assert!(router.quota_resume_ms(d, 0, 0).is_err());
    assert!(router.stop_stalled(d).is_err());
    assert!(router.watch_wait(d, None).is_err());
    assert!(router.retire_delivery(d).is_err());
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "execute",
            "observations",
            "reconcile",
            "reconcile_cancellation",
            "look",
            "probe",
            "resume_after_quota",
            "quota_resume_ms",
            "stop_stalled",
            "watch_wait",
            "retire_delivery"
        ]
        .map(|method| (selected, method, d.key.clone()))
    );
    calls.lock().unwrap().clear();
}

#[test]
fn scenario_shards_cover_every_worker_fixture_group_and_marker_once() {
    use support::scenario_shard::WORKER_GROUPS;
    let groups = WORKER_GROUPS;
    let mut owners = vec![0; groups.len()];
    let mut loads = Vec::new();
    for index in 0..8 {
        let mut shard = support::scenario_shard::Shard::new(index, 8);
        let mut load = 0;
        for (group_index, group) in groups.iter().copied().enumerate() {
            shard.run(group, || {
                owners[group_index] += 1;
                load += group.markers();
            });
        }
        shard.finish("fixture");
        loads.push(load);
    }
    assert!(owners.iter().all(|owners| *owners == 1), "{owners:?}");
    assert_eq!(
        loads.iter().sum::<usize>(),
        groups.iter().map(|group| group.markers()).sum::<usize>()
    );
    assert!(loads.iter().all(|load| *load > 0), "{loads:?}");
}

#[test]
fn scenario_shards_separate_the_measured_worker_hotspots() {
    use support::scenario_shard::{Shard, WORKER_GROUPS, WorkerGroup};

    // Wave-3 BuildBuddy child 705afa47: these three cases shared a 602 s shard.
    let seconds = |group: WorkerGroup| match group {
        WorkerGroup::MakerChecksFail => 242,
        WorkerGroup::Restart => 187,
        WorkerGroup::MakerCiFailure => 84,
        // The seven other 11-marker shards averaged 7 s per marker.
        _ => 7 * group.markers(),
    };
    // #742: Shard::new reads measured weights; feed these estimates to the
    // same partition directly.
    let groups: Vec<_> = WORKER_GROUPS
        .iter()
        .map(|group| (*group, group.markers(), seconds(*group)))
        .collect();
    let mut loads = [0; 8];
    let mut owners = Vec::new();
    for (index, load) in loads.iter_mut().enumerate() {
        let mut shard = Shard::with_groups(index, 8, &groups);
        for group in WORKER_GROUPS.iter().copied() {
            shard.run(group, || {
                *load += seconds(group);
                if matches!(
                    group,
                    WorkerGroup::MakerChecksFail
                        | WorkerGroup::Restart
                        | WorkerGroup::MakerCiFailure
                ) {
                    owners.push(index);
                }
            });
        }
        shard.finish("fixture");
    }
    assert!(
        loads.iter().all(|load| *load <= 300),
        "estimated seconds per shard: {loads:?}"
    );
    owners.sort_unstable();
    owners.dedup();
    assert_eq!(
        owners.len(),
        3,
        "expensive cases must run on distinct shards"
    );
}

#[test]
fn scenario_shard_ownership_is_independent_of_declaration_order() {
    use support::scenario_shard::{Shard, WORKER_GROUPS};

    for total in [1, 2, 8] {
        for index in 0..total {
            let mut forward = Vec::new();
            let mut reverse = Vec::new();
            let mut shard = Shard::new(index, total);
            for group in WORKER_GROUPS.iter().copied() {
                shard.run(group, || forward.push(group));
            }
            shard.finish("fixture");
            let mut shard = Shard::new(index, total);
            for group in WORKER_GROUPS.iter().copied().rev() {
                shard.run(group, || reverse.push(group));
            }
            shard.finish("fixture");
            reverse.reverse();
            assert_eq!(forward, reverse, "shard {index}/{total}");
        }
    }
}

#[test]
#[should_panic(expected = "duplicate scenario group")]
fn scenario_shards_reject_a_duplicated_group_without_a_global_total() {
    let mut shard = support::scenario_shard::Shard::new(0, 1);
    shard.run(support::scenario_shard::WorkerGroup::Router, || {});
    shard.run(support::scenario_shard::WorkerGroup::Router, || {});
}

#[test]
#[should_panic(expected = "missing scenario group")]
fn scenario_shards_reject_a_missing_group_without_a_global_total() {
    let mut shard = support::scenario_shard::Shard::new(0, 1);
    shard.run(support::scenario_shard::WorkerGroup::Router, || {});
    shard.finish("fixture");
}

#[test]
fn controller_restart_waits_for_the_old_postgres_session_and_releases_its_probe() {
    use std::{sync::mpsc, thread, time::Duration, time::Instant};

    let mut database = support::Database::start();
    let settings = database.create("restart_ownership");
    let connect = || {
        postgres::Config::new()
            .host(&settings.host)
            .port(settings.port)
            .user(&settings.user)
            .dbname(&settings.database)
            .password("tl3-disposable-test-password")
            .connect(postgres::NoTls)
            .unwrap()
    };
    let old_owner = tl3_controller::Ownership::claim(Store::open(&settings).unwrap()).unwrap();
    let mut observer = connect();
    let mut waiter = connect();
    let waiter_pid: i32 = waiter
        .query_one("SELECT pg_backend_pid()", &[])
        .unwrap()
        .get(0);
    let (finished, completion) = mpsc::channel();
    let waiting = thread::spawn(move || {
        support::controller_restart::wait_for_owner_release(&mut waiter);
        finished.send(()).unwrap();
        waiter
    });
    let started = Instant::now();
    let returned_early = loop {
        if completion.try_recv().is_ok() {
            break true;
        }
        let blockers: Vec<i32> = observer
            .query_one("SELECT pg_blocking_pids($1)", &[&waiter_pid])
            .unwrap()
            .get(0);
        if !blockers.is_empty() {
            break false;
        }
        // #673 measured this complete restart test at 7.029 s; 180 s > 2x.
        assert!(started.elapsed() < Duration::from_secs(180));
        thread::sleep(Duration::from_millis(10));
    };
    drop(old_owner);
    let waiter = waiting.join().unwrap();
    let claim = Store::open(&settings).and_then(tl3_store::Ownership::claim);
    drop(waiter);
    drop(observer);
    database.stop();
    assert!(
        !returned_early,
        "restart continued while the old PostgreSQL session still owned the controller"
    );
    claim.expect("the synchronization probe must release ownership before restarting");
}

#[test]
fn scenario_shards_cover_nonworker_groups_including_markerless_setup_once() {
    use support::scenario_shard::Shard;
    let groups = [
        ("fast", 1, 10),
        ("slow", 1, 240),
        ("setup", 0, 230),
        ("after_setup", 1, 10),
    ];
    let mut seen = Vec::new();
    let mut loads = [0; 2];
    for (index, load) in loads.iter_mut().enumerate() {
        let mut shard = Shard::with_groups(index, 2, &groups);
        for (group, _, seconds) in groups {
            shard.run(group, || {
                seen.push(group);
                *load += seconds;
            });
        }
        shard.finish("nonworker fixture");
    }
    seen.sort_unstable();
    assert_eq!(seen, ["after_setup", "fast", "setup", "slow"]);
    assert!(loads.iter().all(|load| *load <= 300), "{loads:?}");
}

#[test]
fn worker_groups_are_weighted_by_their_measured_seconds() {
    use support::scenario_shard::{WORKER_GROUPS, measured, weights_ms};
    let groups = measured("tl3-controller/workers");
    let names: Vec<_> = WORKER_GROUPS.iter().map(|group| group.name()).collect();
    assert!(
        !groups.is_empty(),
        "libtest-shards.json has measured worker groups"
    );
    for name in groups.keys() {
        assert!(
            names.contains(&name.as_str()),
            "{name} is measured but is not a worker group; regenerate the weights"
        );
    }
    for (name, weight) in names.iter().zip(weights_ms(&groups, &names)) {
        if let Some(seconds) = groups.get(*name) {
            assert_eq!(
                weight,
                (seconds.as_f64().unwrap() * 1000.0).round() as usize,
                "{name}"
            );
        }
    }
}

#[test]
fn an_unmeasured_group_counts_as_the_median_measured_group() {
    use support::scenario_shard::weights_ms;
    let measured = serde_json::json!({"a": 1.0, "b": 2.5, "slow": 90.0});
    let weights = weights_ms(measured.as_object().unwrap(), &["slow", "a", "new"]);
    assert_eq!(weights, [90_000, 1_000, 2_500]);
}
