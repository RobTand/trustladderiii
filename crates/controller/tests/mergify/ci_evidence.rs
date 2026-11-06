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
    "multiple-drafts",
    "truncated",
    "range-truncated",
    "ansi-mixed",
    "controls",
    "compile",
    "lint",
    "redelivery",
    "diagnostics-golden",
    "diagnostics-feedback",
    "diagnostics-categories",
    "diagnostics-rust",
    "diagnostics-limits",
    "diagnostics-hostile",
];
const TEST: &str = "//tests:test_a_ready_gpu_action_is_not_starved_by_cpu_shards";
const URL: &str =
    "https://github.com/RobTand/prismabuild/actions/runs/41524906888/job/125550020549";

pub(super) struct Fixture {
    pull: Value,
    checks: Value,
    files: Value,
    log: String,
    gate_log: Option<String>,
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
                return Some(ResponseTemplate::new(200).set_body_string(self.gate_log.as_deref().unwrap_or("Gate verdict: failed")));
            }
            "/repos/owner/sandbox/actions/jobs/125550020549/logs" => {
                if self.log == "range-truncated" {
                    let log = format!(
                        "old fragment\n{}",
                        include_str!("../fixtures/mergify/pb2625/gate.log")
                    );
                    let total = 90000 + log.len();
                    return Some(
                        ResponseTemplate::new(206)
                            .insert_header(
                                "content-range",
                                format!("bytes 90000-{}/{}", total - 1, total),
                            )
                            .set_body_string(log),
                    );
                }
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

// #1574/431: only these public diagnostic lines are retained. Neutral padding
// preserves their byte offsets, line numbers, ordering and distance from EOF.
const REPAIR: &str = "Stale generated inputs, parity drift or growing budgets; fix with tools/bazel/generate.sh: python3.14 tools/ci/full_shards.py --check";
const GATE: &str = "The Bazel gate did not finish detection and scope upload: {'result': 'failure', 'outputs': {'scopes_uploaded': 'true', 'tests': 'failure'}}";
fn retained_pattern(line: &str, offset: usize, number: usize, size: usize) -> String {
    let mut log = "neutral log line\n".repeat(number - 2);
    log.push_str(&" ".repeat(offset - log.len() - 1));
    log.push('\n');
    log.push_str("2026-11-06T22:40:52.7191760Z ");
    log.push_str(line);
    log.push_str("\n2026-11-06T22:40:52.7325037Z ##[error]Process completed with exit code 1.\n");
    let padding = size - log.len();
    log.push_str(&"neutral cleanup\n".repeat(padding / 16));
    log.push_str(&" ".repeat(size - log.len()));
    assert_eq!(log.len(), size);
    log
}
fn assert_golden(text: &str) {
    let missing: Vec<_> = [REPAIR, GATE].into_iter().filter(|line| !text.contains(line)).collect();
    assert!(missing.is_empty(), "missing diagnostics {missing:?}: {text}");
    assert!(!text.contains("full target lists differ from live query"));
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
    if mode == "multiple-drafts" {
        check["output"]["summary"] = json!(format!(
            "Previous failed attempt: draft #9999\n{}",
            check["output"]["summary"].as_str().unwrap()
        ));
    }
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
        "diagnostics-golden" | "diagnostics-feedback" => {
            log = retained_pattern(REPAIR, 47048, 490, 68911);
        }
        "diagnostics-categories" => {
            log = "::error::annotation diagnostic\nTraceback (most recent call last):\n  File \"runner.py\", line 7, in <module>\n    run()\nValueError: invalid fixture inventory\nERROR: /workspace/BUILD:7: missing input\ngate failed; fix with tools/regenerate.sh\n::error::annotation diagnostic\nFAILED: //tests:test_action_resource_profile\n".into();
            log.push_str(&"neutral cleanup\n".repeat(200));
        }
        "diagnostics-rust" => {
            log = "---- resources::budget stdout ----\nthread 'resources::budget' panicked at src/resources.rs:42:9:\nassertion `left == right` failed: CPU reservation exceeded\n  left: 8\n right: 4\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\nFAILED (rust): //tests:test_action_resource_profile\n".into();
            log.push_str(&"neutral cleanup\n".repeat(200));
        }
        "diagnostics-limits" => {
            log = format!("::error::{}\n", "雪".repeat(1000));
            for index in 0..30 {
                log.push_str(&format!("ERROR: diagnostic {index}\n"));
            }
            log.push_str("FAILED: //tests:test_action_resource_profile\n");
            log.push_str(&"neutral cleanup\n".repeat(200));
        }
        "diagnostics-hostile" => {
            log = "\x1b]8;;https://example.invalid\x07\x1b[31m::error::pass\x1b[0mword=planted-password Bearer planted-bearer\x1b]8;;\x1b\\\nERROR: ghp_planted123456789\0\x08\x7f\nFAILED: //tests:test_action_resource_profile\n".into();
            log.push_str(&"neutral cleanup\n".repeat(200));
        }
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
        "truncated" => {
            log = format!(
                "FAILED: //tests:test_action_resource_profile\n{}\n{log}",
                "old line\n".repeat(10000)
            );
        }
        "range-truncated" => log = "range-truncated".into(),
        "ansi-mixed" => {
            log = format!("\x1b[31mFAILED: //tests:test_action_resource_profile\x1b[0m\n{log}");
        }
        "controls" => {
            log = "\x1b]8;;https://example.invalid\x07\x1b[31mFAILED: //tests:test_action_resource_profile\x1b[0m\x1b]8;;\x1b\\ pass\x1b[32mword=planted-password\x1b[0m\nBearer planted-bearer\0\x08\x7f\u{009b}31m\u{009c}\n".into();
        }
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
        gate_log: matches!(mode, "diagnostics-golden" | "diagnostics-feedback")
            .then(|| retained_pattern(GATE, 11199, 164, 13457)),
    });
    if matches!(mode, "aggregate" | "compile" | "lint" | "diagnostics-golden" | "diagnostics-feedback") {
        state.ci_fixture.as_mut().unwrap().checks["check_runs"].as_array_mut().unwrap().push(json!({
            "id":125552384811_u64,"name":"gate","head_sha":gate["head"],"status":"completed","conclusion":"failure",
            "app":{"id":15368,"slug":"github-actions"},"output":{"title":"Gate verdict failed","summary":"Bazel failed"}
        }));
    }
    if matches!(mode, "compile" | "lint") {
        let extra = &mut state.ci_fixture.as_mut().unwrap().checks["check_runs"][1];
        extra["name"] = json!(mode);
        extra["output"]["summary"] = json!(format!("{mode} failed without test IDs"));
    }
    if mode == "controls" {
        let check = &mut state.ci_fixture.as_mut().unwrap().checks["check_runs"][0];
        check["name"] = json!("\x1b[31mBazel\x1b[0m");
        check["details_url"] = json!(format!("\x1b[32m{URL}\x1b[0m"));
        check["output"]["title"] = json!("\x1b[31mGate failed\x1b[0m");
        check["output"]["summary"] = json!("pass\x1b[0mword=planted-password\0\x08\x7f");
    }
}
fn observe(controller: &mut Controller, api: &Server, config: &Bootstrap, head_check: bool) {
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
    for mut input in inputs {
        if head_check
            && let tl3_controller::cli::Input::Observation {
                observation: tl3_controller::executor::Observation::Queue(fact), ..
            } = &mut input
            && let adapters::mergify::Evidence::CiFailed { head_check, infra, .. } = &mut fact.evidence
        {
            *head_check = true;
            *infra = false;
        }
        ingress::apply(controller, input).unwrap();
    }
    for _ in 0..20 {
        scenario_progress("ci_evidence: controller step");
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
    // CI evidence does not exercise allowance timers. Keep fixture work from
    // accidentally triggering liveness probes when an executor is busy.
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = queued_for_clock(
        settings,
        &api,
        root.path(),
        true,
        &[],
        config.clone(),
        Some(clock.clone()),
    );
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
    observe(&mut controller, &api, &config, mode == "diagnostics-feedback");
    if matches!(mode, "unparsed" | "logs-unavailable" | "diagnostics-golden") {
        // #854b: the first opaque refusal is a Q50 hold, even if fail_fast
        // would otherwise close the fixture. It never spends a failed round.
        let after = controller.world().unwrap().nodes[0];
        assert_eq!(after.phase, Phase::Queued);
        assert_eq!(after.run, before.run);
        assert_eq!(after.round, before.round);
        assert_eq!(after.attempt, before.attempt);
        assert_eq!(after.credit.spent, before.credit.spent);
        assert_eq!(api.state.lock().unwrap().requeues, 0);
        let facts = queue_facts(&mut controller);
        let details = &facts.last().unwrap()["evidence"]["CiFailed"]["details"];
        assert_eq!(details["cause"], "Unattributed");
        if mode == "diagnostics-golden" {
            assert_eq!(details["complete"], false);
            assert!(details["failed_tests"].as_array().unwrap().is_empty());
            let summaries = details["checks"].as_array().unwrap().iter()
                .map(|check| check["summary"].as_str().unwrap()).collect::<Vec<_>>().join("\n");
            assert_golden(&summaries);
        }
        assert_eq!(details["checks"][0]["name"], "Bazel");
        assert!(
            controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.table == "queue_holds")
        );
        println!("test ci_evidence_{mode} ... ok");
        return;
    }
    if matches!(
        mode,
        "outside" | "repeat" | "multiple-drafts" | "redelivery"
    ) {
        let after = controller.world().unwrap().nodes[0];
        assert_ne!(
            after.phase,
            Phase::Making,
            "outside test failure must not start a maker"
        );
        assert_eq!(after.run, before.run, "no maker run is started");
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
        let facts = queue_facts(&mut controller);
        let decision = facts.last().unwrap();
        assert_eq!(decision["evidence"]["CiFailed"]["infra"], true);
        assert_eq!(
            decision["evidence"]["CiFailed"]["details"]["decision"],
            "outside_change"
        );
        if !matches!(mode, "repeat" | "redelivery") {
            println!("test ci_evidence_{mode} ... ok");
            return;
        }
        // Restart before the repeated failure; retry history must be durable.
        drop(controller);
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Delivery,
            api.adapter(&root.path().join("mergify"), 0)
                .with_store(settings.clone())
                .with_clock(clock.clone()),
        );
        controller = Controller::open_with_clock(
            Store::open(settings).unwrap(),
            registry,
            config.clone(),
            clock.clone(),
        )
        .unwrap();
        if mode == "redelivery" {
            failure(&api, mode, false);
            let before = api.state.lock().unwrap().get_paths.clone();
            observe(&mut controller, &api, &config, mode == "diagnostics-feedback");
            let snapshot = controller.snapshot().unwrap();
            let changes = serde_json::from_value(json!({"repository":"RobTand/prismabuild", "issues":[],
                "comments":[], "labels":[], "pull_requests":[], "check_runs":[], "check_suites":[]})).unwrap();
            Translator::new(api.clients())
                .unwrap()
                .catch_up(&config.repos[0], &changes, &snapshot)
                .unwrap();
            let mut delivery = delivery();
            delivery.repository = config.repos[0].clone();
            delivery.pull_request = Some(100);
            delivery.head_sha = Some(
                api.state.lock().unwrap().pull["head"]["sha"]
                    .as_str()
                    .unwrap()
                    .into(),
            );
            api.adapter(&root.path().join("redelivered"), 0)
                .with_store(settings.clone())
                .observations(&delivery)
                .unwrap();
            let after = api.state.lock().unwrap().get_paths.clone();
            for path in [
                "/repos/owner/sandbox/pulls/2661",
                "/repos/owner/sandbox/pulls/100/files",
                "/repos/owner/sandbox/commits/8e8df7e2fedfa413a7c6698ba1749a340d6bcb28/check-runs",
                "/repos/owner/sandbox/actions/jobs/125550020549/logs",
            ] {
                assert_eq!(
                    after.get(path),
                    before.get(path),
                    "redelivery must not recollect {path}"
                );
            }
            assert_eq!(api.state.lock().unwrap().requeues, 1);
            println!("test ci_evidence_{mode} ... ok");
            return;
        }
        failure(&api, mode, true);
        observe(&mut controller, &api, &config, mode == "diagnostics-feedback");
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
    if !matches!(mode, "unparsed" | "logs-unavailable" | "diagnostics-feedback") {
        assert!(!finding["failed_tests"].as_array().unwrap().is_empty());
    }
    if mode.starts_with("diagnostics-") {
        let summaries = finding["ci_checks"].as_array().unwrap().iter()
            .map(|check| check["summary"].as_str().unwrap()).collect::<Vec<_>>().join("\n");
        assert!(report.chars().count() <= 16000);
        for check in finding["ci_checks"].as_array().unwrap() {
            assert!(check["summary"].as_str().unwrap().chars().count() <= 1000);
        }
        match mode {
            "diagnostics-feedback" => {
                assert_golden(report);
                assert_golden(&summaries);
                assert!(finding["failed_tests"].as_array().unwrap().is_empty());
            }
            "diagnostics-categories" => {
                let mut last = 0;
                for fragment in ["::error::annotation diagnostic", "Traceback (most recent call last):", "ValueError: invalid fixture inventory", "ERROR: /workspace/BUILD:7: missing input", "gate failed; fix with tools/regenerate.sh"] {
                    let index = summaries.find(fragment).unwrap_or_else(|| panic!("missing {fragment}: {summaries}"));
                    assert!(index >= last, "source order: {summaries}");
                    assert_eq!(summaries.matches(fragment).count(), 1, "dedupe {fragment}");
                    last = index;
                }
                assert!(summaries.contains("FAILED: //tests:test_action_resource_profile"));
                let facts = queue_facts(&mut controller);
                assert_eq!(facts.last().unwrap()["evidence"]["CiFailed"]["details"]["complete"], true);
            }
            "diagnostics-rust" => {
                for fragment in ["panicked at src/resources.rs:42:9", "CPU reservation exceeded", "left: 8", "right: 4"] {
                    assert!(summaries.contains(fragment), "missing {fragment}: {summaries}");
                }
            }
            "diagnostics-limits" => {
                assert!(summaries.contains("[diagnostics omitted]"), "{summaries}");
                assert!(summaries.contains("[diagnostic clipped]"), "{summaries}");
                assert!(summaries.len() <= 1000);
                assert!(summaries.lines().filter(|line| line.starts_with("ERROR:")).count() <= 8);
                assert!(summaries.contains("FAILED: //tests:test_action_resource_profile"));
            }
            "diagnostics-hostile" => {
                assert!(summaries.contains("::error::"));
                for secret in ["planted-password", "planted-bearer", "ghp_planted123456789"] {
                    assert!(!finding.to_string().contains(secret));
                }
                assert!(summaries.chars().all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t')));
            }
            _ => unreachable!(),
        }
    }
    if mode == "repeat" {
        assert!(finding.to_string().contains(TEST));
        let facts = queue_facts(&mut controller);
        let repeated = facts
            .iter()
            .find(|fact| fact["evidence"]["CiFailed"]["details"]["decision"] == "repeated_test")
            .unwrap();
        assert_eq!(repeated["evidence"]["CiFailed"]["infra"], false);
    }
    if mode == "ansi-mixed" {
        assert_eq!(
            finding["failed_tests"].as_array().unwrap().len(),
            2,
            "coloured failures must not disappear"
        );
    }
    if matches!(
        mode,
        "truncated" | "range-truncated" | "aggregate" | "compile" | "lint"
    ) {
        let facts = queue_facts(&mut controller);
        assert_eq!(
            facts.last().unwrap()["evidence"]["CiFailed"]["details"]["complete"],
            false
        );
    }
    if mode == "controls" {
        for fact in queue_facts(&mut controller) {
            let details = &fact["evidence"]["CiFailed"]["details"];
            if details.is_object() {
                for check in details["checks"].as_array().unwrap() {
                    for field in ["name", "summary", "url"] {
                        assert!(
                            check[field]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
                        );
                    }
                }
                for test in details["failed_tests"].as_array().unwrap() {
                    for field in ["test", "message"] {
                        assert!(
                            test[field]
                                .as_str()
                                .unwrap()
                                .chars()
                                .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
                        );
                    }
                }
            }
        }
    }
    if mode == "overflow" {
        assert_eq!(finding["failed_tests"].as_array().unwrap().len(), 20);
    }
    if matches!(mode, "hostile" | "controls") {
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
