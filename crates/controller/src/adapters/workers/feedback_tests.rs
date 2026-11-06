use super::*;
use crate::{Bootstrap, Controller, LabelledIssue, git_tests::Repo, store_support::Database};
use tl3_kernel::{NodeId, Phase};
use tl3_store::Commit;

struct Fixture {
    controller: Controller,
    supervisor: Supervisor,
    config: tl3_config::Config,
    repo: Repo,
    _db: Database,
}

impl Fixture {
    fn new() -> Self {
        let repo = Repo::new();
        let mut db = Database::start();
        let mut config = tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
        config.store = db.create("step_feedback");
        config.workers.runs = repo.root.path().join("runs");
        let supervisor = Supervisor::new(&config, &repo.root.path().join("records"), &mut crate::store_support::ownership()).unwrap();
        let mut controller = Controller::open(Store::open(&config.store).unwrap(), crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(), Bootstrap::from_config(&config).unwrap()).unwrap();
        controller.intake(LabelledIssue {
            delivery: "feedback".into(), owner: "RobTand".into(), repo: "tl3-sandbox".into(), installation: 1,
            number: 42, title: "Repair the failing check".into(), actor: config.github.rob.id,
            label: "tl3".into(), labels: vec![],
        }).unwrap();
        controller.store.put_worker_materials("0", &json!({"maker": {
            "brief":"Return hello.", "goals":[{"id":"greeting","description":"Return hello.","verification":"test"}],
            "plan":{"files":["README.md"],"checks":[{"name":"hello","kind":"demonstrating","tests":["test_greeting.py::test_hello"],"goal_ids":["greeting"]}]}
        }})).unwrap();
        Self { controller, supervisor, config, repo, _db: db }
    }

    fn seed(&mut self, phase: Phase, pos: u8) {
        let snapshot = self.controller.snapshot().unwrap();
        let mut world = self.controller.world().unwrap();
        let footprint = world.intern_footprint([tl3_kernel::FileId(0)]);
        let n = &mut world.nodes[0];
        n.phase = phase;
        n.pos = pos;
        n.head = Some(self.controller.head(NodeId(0), &self.repo.head).unwrap());
        n.generation = 10;
        n.run = matches!(phase, Phase::Making | Phase::Reviewing | Phase::Planning { .. }).then_some(10);
        n.job = (phase == Phase::Checking).then_some(10);
        n.effect = None;
        n.pool = n.run.is_some();
        n.holds = !matches!(phase, Phase::Planning { .. });
        n.footprint = footprint;
        n.checked = n.head;
        n.approved = n.head;
        n.pr_open = matches!(phase, Phase::Publishing | Phase::Queued);
        n.had_pr = n.pr_open;
        n.paused = true;
        n.ticket = None;
        n.round = 0;
        n.attempt = 1;
        n.credit.start = 100;
        n.credit.ready = true;
        assert!(world.valid(), "seed must be a valid settled world: {:?}", world.check());
        let mut commit = Commit::new("feedback-seed", json!(null), &snapshot);
        commit.rows.extend(crate::snapshot::changes(&crate::snapshot::encode(&self.controller.world().unwrap()).unwrap(), crate::snapshot::encode(&world).unwrap()));
        self.controller.store.commit(&commit).unwrap();
    }

    fn observe(&mut self, observation: Observation) {
        self.controller.observe("failure", observation).unwrap();
        assert!(self.controller.drive_once().unwrap());
    }

    fn maker(&mut self) -> Value {
        let snapshot = self.controller.snapshot().unwrap();
        let n = self.controller.world().unwrap().nodes[0];
        let mut delivery = self.repo.delivery();
        delivery.generation = n.generation;
        delivery.attempt = n.attempt;
        delivery.round = n.round;
        self.supervisor.build_request(&delivery, "maker", &mut Store::open(&self.config.store).unwrap(), &snapshot).unwrap()["inputs"].clone()
    }
}

#[test]
fn publishing_enqueue_without_inflight_effect_delivers_ci_feedback() {
    let mut f = Fixture::new();
    // #862: the kernel agenda is at Enqueue, but there is no in-flight effect.
    f.seed(Phase::Publishing, 3);
    f.observe(Observation::CiFailed {
        node: NodeId(0), infra: false,
        checks: vec![crate::findings::CiFailure { name: "test_greeting".into(), summary: "expected hello, got goodbye".into(), url: Some("https://ci.invalid/check/42".into()) }],
    });
    assert_eq!(f.controller.world().unwrap().nodes[0].round, 1);
    let inputs = f.maker();
    assert!(inputs["feedback"].to_string().contains("expected hello, got goodbye"), "the D2 repair maker needs the failing check: {inputs}");
    assert!(f.controller.snapshot().unwrap().rows.iter().any(|r| r.table == "maker_findings" || r.table == "feedback"));
}

#[test]
fn failed_run_details_reach_the_retry_maker() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    f.observe(Observation::WorkerFailed {
        node: NodeId(0), generation: 10, failure_class: "worker_exit".into(), failure_message: "maker exited during reproduction".into(),
    });
    let inputs = f.maker();
    assert!(inputs["feedback"].to_string().contains("maker exited during reproduction"), "retry needs its failure evidence: {inputs}");
}
