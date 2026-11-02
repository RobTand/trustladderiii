use super::*;

pub(super) const MODES: &[&str] = &[
    "outside",
    "repeat",
    "changed",
    "named",
    "unparsed",
    "hostile",
    "unknown",
    "mixed",
    "pytest",
    "tl3",
    "aggregate",
    "check-name",
    "overflow",
    "renamed",
    "logs-unavailable",
    "missing-plan",
];
const TEST: &str = "//tests:test_a_ready_gpu_action_is_not_starved_by_cpu_shards";
const URL: &str =
    "https://github.com/RobTand/prismabuild/actions/runs/41524906888/job/125550020549";

pub(super) struct Fixture {
    pull: Value,
    checks: Value,
    files: Value,
    log: String,
}
impl Fixture {
    pub(super) fn respond(&self, path: &str) -> Option<ResponseTemplate> {
        let body = match path {
            "/repos/owner/sandbox/pulls/2661" => &self.pull,
            "/repos/owner/sandbox/pulls/100/files" => &self.files,
            "/repos/owner/sandbox/commits/8e8df7e2fedfa413a7c6698ba1749a340d6bcb28/check-runs" => {
                &self.checks
            }
            "/repos/owner/sandbox/actions/jobs/125552384811/logs" => {
                return Some(ResponseTemplate::new(200).set_body_string("Gate verdict: failed"));
            }
            "/repos/owner/sandbox/actions/jobs/125550020549/logs" => {
                if self.log == "unavailable" {
                    return Some(ResponseTemplate::new(403));
                }
                return Some(ResponseTemplate::new(200).set_body_string(&self.log));
            }
            _ => return None,
        };
        Some(ResponseTemplate::new(200).set_body_json(body))
    }
}
fn failure(api: &Server, mode: &str, repeat: bool) {
    let gate: Value =
        serde_json::from_str(include_str!("../fixtures/mergify/pb2625/gate-result.json")).unwrap();
    let mut state = api.state.lock().unwrap();
    let mut check: Value =
        serde_json::from_str(include_str!("../fixtures/mergify/pb2625/queue-check.json")).unwrap();
    check["head_sha"] = state.pull["head"]["sha"].clone();
    check["completed_at"] = json!(if repeat {
        "2026-11-02T21:20:00Z"
    } else {
        "2026-11-02T21:09:00Z"
    });
    state.checks = json!([check]);
    state.queue_rule["completed_at"] = json!("2026-11-02T21:30:00Z");
    let mut draft = state.pull.clone();
    draft["number"] = json!(2661);
    draft["user"]["login"] = json!("mergify[bot]");
    draft["head"]["ref"] = json!("mergify/merge-queue/main/pr-2660");
    draft["head"]["sha"] = gate["head"].clone();
    let mut log = include_str!("../fixtures/mergify/pb2625/gate.log").to_owned();
    let mut files = json!([{"filename":gate["changed"][0]}]);
    match mode {
        "changed" => {
            files = json!([{"filename":"tests/test_a_ready_gpu_action_is_not_starved_by_cpu_shards.py"}])
        }
        "renamed" => {
            files = json!([{
                "filename":"tests/renamed.py",
                "previous_filename":"tests/test_a_ready_gpu_action_is_not_starved_by_cpu_shards.py"
            }])
        }
        "logs-unavailable" => log = "unavailable".into(),
        "overflow" => {
            log = (0..30)
                .map(|index| format!("FAILED: //tests:test_outside_{index}\n"))
                .collect()
        }
        "unparsed" => log = "runner exited without any test IDs".into(),
        "unknown" => log = "FAILED: //somewhere:unmapped".into(),
        "mixed" => log.push_str("FAILED: //tests:test_action_resource_profile\n"),
        "pytest" => {
            log = "FAILED tests/test_action_resource_profile.py::test_resources - mismatch\n".into()
        }
        "tl3" => {
            log = "FAILED (rust): //crates/controller:test_test_workers\n".into();
            files = json!([{"filename":"crates/controller/tests/workers.rs"}]);
        }
        "hostile" => {
            log = "FAILED: //tests:test_action_resource_profile password=planted-password\n"
                .repeat(30);
            log.push_str(&format!(
                "{}\nBearer planted-bearer\nghp_planted123456789\n",
                "雪".repeat(2000)
            ));
        }
        _ => {}
    }
    let summary = if mode == "hostile" {
        "password=planted-password"
    } else {
        "Bazel tests failed"
    };
    state.ci_fixture = Some(Fixture {
        pull: draft,
        checks: json!({"check_runs":[{"id":125550020549_u64,"name":"Bazel","head_sha":gate["head"],
            "status":"completed","conclusion":"failure","details_url":URL,
            "app":{"id":15368,"slug":"github-actions"},
            "output":{"title":"Gate failed","summary":summary}}]}),
        files,
        log,
    });
    if mode == "aggregate" {
        state.ci_fixture.as_mut().unwrap().checks["check_runs"].as_array_mut().unwrap().push(json!({
            "id":125552384811_u64,"name":"gate","head_sha":gate["head"],"status":"completed","conclusion":"failure",
            "app":{"id":15368,"slug":"github-actions"},"output":{"title":"Gate verdict failed","summary":"Bazel failed"}
        }));
    }
}
fn observe(controller: &mut Controller, api: &Server, config: &Bootstrap) {
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    let inputs = Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed("check_run", body),
            &controller.snapshot().unwrap(),
            &config.repos,
        )
        .unwrap();
    for input in inputs {
        ingress::apply(controller, input).unwrap();
    }
    for _ in 0..20 {
        controller.drive_once().unwrap();
        if controller.world().unwrap().nodes[0].phase != Phase::Queued {
            break;
        }
    }
}
pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let mut config = bootstrap();
    config.repos[0].owner = "RobTand".into();
    config.repos[0].name = if mode == "tl3" {
        "trustladderiii"
    } else {
        "prismabuild"
    }
    .into();
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued_for(settings, &api, root.path(), true, &[], config.clone());
    if mode != "missing-plan" {
        let snapshot = controller.snapshot().unwrap();
        let head = api.state.lock().unwrap().pull["head"]["sha"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut commit = tl3_store::Commit::new("named-ci-check", json!({}), &snapshot);
        commit.rows.push(tl3_store::RowChange::Put {table:"check_plans".into(),id:format!("0:{head}"),value:json!({
            "repository":"/fixture", "base_sha":"566bee93ba4b52681864132960cfd63e36bcc9cd", "plan":if matches!(mode, "named" | "check-name") {json!([{
                "name":if mode == "check-name" {"Bazel"} else {"gpu fairness"}, "kind":"regression", "tests":if mode == "check-name" {json!(["tests/a.py::test_a"])} else {json!([TEST])}}])} else {json!([])}})});
        Store::open(settings).unwrap().commit(&commit).unwrap();
    }
    let before = controller.world().unwrap().nodes[0];
    failure(&api, mode, false);
    observe(&mut controller, &api, &config);
    if matches!(mode, "outside" | "repeat" | "aggregate") {
        let after = controller.world().unwrap().nodes[0];
        assert_ne!(
            after.phase,
            Phase::Making,
            "outside test failure must not start a maker"
        );
        assert_eq!(after.credit.spent, before.credit.spent);
        assert_eq!(after.attempt, before.attempt);
        assert_eq!(after.round, before.round);
        drain(&mut controller);
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
        assert_eq!(api.state.lock().unwrap().requeues, 1, "one free requeue");
        assert!(
            json!(queue_facts(&mut controller))
                .to_string()
                .contains(TEST),
            "journal must retain the decision's test IDs"
        );
        if mode != "repeat" {
            println!("test ci_evidence_{mode} ... ok");
            return;
        }
        // Restart before the repeated failure; retry history must be durable.
        drop(controller);
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Mergify,
            api.adapter(&root.path().join("mergify"), 0),
        );
        controller =
            Controller::open(Store::open(settings).unwrap(), registry, config.clone()).unwrap();
        failure(&api, mode, true);
        observe(&mut controller, &api, &config);
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Making,
        "{mode} needs a normal fix round"
    );
    let findings = Store::open(settings)
        .unwrap()
        .worker_document("0", "maker_findings")
        .unwrap()
        .unwrap();
    let finding = findings.as_array().unwrap().last().unwrap();
    assert_eq!(finding["ci_checks"][0]["name"], "Bazel");
    assert_eq!(finding["ci_checks"][0]["url"], URL);
    let report = finding["report"].as_str().unwrap();
    assert!(!report.contains("no check details"));
    assert!(report.contains("Gate failed"));
    if !matches!(mode, "unparsed" | "logs-unavailable") {
        assert!(!finding["failed_tests"].as_array().unwrap().is_empty());
    }
    if mode == "repeat" {
        assert!(finding.to_string().contains(TEST));
    }
    if mode == "overflow" {
        assert_eq!(finding["failed_tests"].as_array().unwrap().len(), 20);
    }
    if mode == "hostile" {
        let text = finding.to_string();
        for secret in ["planted-password", "planted-bearer", "ghp_planted123456789"] {
            assert!(!text.contains(secret), "must redact {secret}");
        }
        assert!(finding["failed_tests"].as_array().unwrap().len() <= 20);
        assert!(report.chars().count() <= 16000);
        for test in finding["failed_tests"].as_array().unwrap() {
            assert!(test["message"].as_str().unwrap().chars().count() <= 1000);
        }
    }
    println!("test ci_evidence_{mode} ... ok");
}
