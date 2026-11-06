use super::*;

pub(super) const MODES: &[&str] = &[
    "repair",
    "queue-running",
    "suite-running",
    "suite-nonempty",
    "suite-unreadable",
    "suite-newer",
    "foreign",
];

pub(super) struct Fixture {
    mode: String,
}

impl Fixture {
    pub(super) fn respond(&self, path: &str) -> Option<ResponseTemplate> {
        if path.contains("/check-suites/") && path.ends_with("/check-runs") {
            return Some(if self.mode == "suite-unreadable" {
                ResponseTemplate::new(503)
            } else if self.mode == "suite-nonempty" {
                // A positive nonzero count must block even if a page has no runs.
                ResponseTemplate::new(200)
                    .set_body_json(json!({"total_count":1,"check_runs":[]}))
            } else {
                ResponseTemplate::new(200).set_body_json(
                    serde_json::from_str::<Value>(include_str!(
                        "../fixtures/mergify/tessera1639/empty-suite.json"
                    ))
                    .unwrap(),
                )
            });
        }
        if path.contains("/actions/jobs/") && path.ends_with("/logs") {
            return Some(ResponseTemplate::new(200).set_body_string(match path {
                p if p.contains("127446883085") => "Detection failed: changed Python files that no Bazel target holds: tests/test_installed_commit_owner.py. Run tools/bazel/generate.sh.",
                p if p.contains("127446882423") => "FAILED tests/test_bazel_build.py::test_the_generated_build_files_are_current\nFAILED tests/test_bazel_build.py::test_every_gate_file_has_a_target_in_each_lane",
                _ => "Bazel detection failed; scopes were not uploaded.",
            }));
        }
        None
    }
}

/// #854 D1/D2, state-table :589/:624: actual check IDs, states, timestamps,
/// queue payload and seven suites. Only transport repo/PR/head identities are
/// rebound to the simulated pipeline's independently checked/approved commit.
pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = queued_for_clock(
        settings, &api, root.path(), true, &[], bootstrap(), Some(clock.clone()),
    );
    let before = controller.world().unwrap().nodes[0];
    {
        let mut state = api.state.lock().unwrap();
        let head = state.pull["head"]["sha"].clone();
        let mut checks: Value = serde_json::from_str(include_str!(
            "../fixtures/mergify/tessera1639/checks.json"
        )).unwrap();
        for check in checks["check_runs"].as_array_mut().unwrap() {
            check["head_sha"] = head.clone();
            check["pull_requests"] = json!([]);
        }
        state.checks = checks["check_runs"].clone();
        state.queue_rule["conclusion"] = json!("success");
        let mut suites: Value = serde_json::from_str(include_str!(
            "../fixtures/mergify/tessera1639/suites.json"
        )).unwrap();
        for suite in suites["check_suites"].as_array_mut().unwrap() {
            suite["head_sha"] = head.clone();
        }
        state.suites = suites["check_suites"].clone();
        state.entry_fixture = Some(Fixture { mode: mode.into() });
        match mode {
            "queue-running" => {
                let check = state.queue_check_mut();
                check["status"] = json!("in_progress");
                check["completed_at"] = Value::Null;
                check["conclusion"] = Value::Null;
            }
            "suite-running" => state.suites[0]["status"] = json!("in_progress"),
            "suite-newer" => state.suites[0]["updated_at"] = json!("2026-11-05T19:00:00Z"),
            "foreign" => {
                for suite in state.suites.as_array_mut().unwrap() {
                    suite["status"] = json!("completed");
                }
            }
            _ => {}
        }
    }
    if mode == "foreign" {
        // No TL3 store/binding: preserve the adapter's existing #447 setup fact.
        let snapshot = controller.snapshot().unwrap();
        let delivery: Delivery = serde_json::from_value(snapshot.rows.iter()
            .find(|row| row.table == "allowances" && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue")
            .unwrap().value["delivery"].clone()).unwrap();
        let observations = api.adapter(&root.path().join("foreign"), 0)
            .observations(&delivery).unwrap();
        assert!(observations.iter().any(|observation| matches!(observation,
            tl3_controller::executor::Observation::Queue(tl3_controller::adapters::mergify::QueueFact {
                evidence: tl3_controller::adapters::mergify::Evidence::QueueBlocked { failed, conditions, .. }, ..
            }) if failed.iter().any(|name| name == "Bazel") && conditions.iter().any(|condition| condition.contains("scopes-available"))
        )));
    } else {
        let mut allowances = tl3_config::Allowances::default();
        allowances.github_mergify.silence_secs = 1;
        allowances.github_mergify.no_progress_secs = 3;
        controller.set_allowances(allowances);
        for _ in 0..12 {
            clock.0.set(clock.0.get() + Duration::from_secs(10));
            controller.look_due().unwrap();
            if controller.world().unwrap().nodes[0].phase == Phase::Making {
                break;
            }
            controller.drive_once().unwrap();
            if controller.world().unwrap().nodes[0].phase == Phase::Making {
                break;
            }
        }
        let node = controller.world().unwrap().nodes[0];
        let snapshot = controller.snapshot().unwrap();
        if mode == "repair" {
            assert_eq!(node.phase, Phase::Making, "#854 must repair, not stall: {}", json!(snapshot.rows.iter().filter(|r| r.table == "incidents" || r.table == "allowances").map(|r| &r.value).collect::<Vec<_>>()));
            assert_eq!(node.round, before.round + 1);
            assert!(!node.waiting_incident);
            let findings = Store::open(settings).unwrap().worker_document("0", "maker_findings").unwrap().unwrap().to_string();
            for expected in ["Bazel", "pure", "pr-gate", "scopes-available", "test_installed_commit_owner.py"] {
                assert!(findings.contains(expected), "missing {expected}: {findings}");
            }
            assert!(!snapshot.rows.iter().any(|r| r.table == "incidents" && r.value["open"] == true));
        } else {
            assert_eq!(node.phase, Phase::Queued, "{mode} must block D1/D2");
            assert_eq!(node.round, before.round);
            assert!(!snapshot.rows.iter().any(|r| r.table == "queue_facts" && (r.value["evidence"].get("CiFailed").is_some() || r.value["evidence"].get("QueueBlocked").is_some())));
            if mode == "queue-running" {
                assert!(snapshot.rows.iter().any(|r| r.table == "allowances" && r.value["failure"] == "queue_unfinished"));
                assert!(node.waiting_incident && node.incident_needs_rob);
            }
        }
    }
    println!("test queue_entry_{mode} ... ok");
}
