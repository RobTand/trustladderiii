use super::*;
use tl3_controller::adapters::mergify::{Evidence, QueueFact};
use tl3_controller::executor::Observation;

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let pending = mode == "reopen-look";
    api.state.lock().unwrap().lag = pending;
    let mut controller = queued(settings, &api, root.path(), !pending);
    if let Some(source) = mode.strip_prefix("reopen-") {
        reopen(&mut controller, &api, source);
    } else if mode == "gaps" {
        for (index, gaps) in [
            vec!["Contents: read unavailable"],
            vec!["Commit statuses: read unavailable"],
            vec![
                "Contents: read unavailable",
                "Commit statuses: read unavailable",
            ],
            vec![
                "Commit statuses: read unavailable",
                "Contents: read unavailable",
            ],
        ]
        .into_iter()
        .enumerate()
        {
            controller
                .observe(
                    &format!("gap-{index}"),
                    Observation::Queue(QueueFact {
                        repository: bootstrap().repos[0].clone(),
                        number: 100,
                        evidence: Evidence::ChecksUnavailable {
                            permissions: gaps.into_iter().map(str::to_owned).collect(),
                        },
                    }),
                )
                .unwrap();
            drain(&mut controller);
        }
        let snapshot = controller.snapshot().unwrap();
        let incidents: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "incidents")
            .collect();
        assert!(
            incidents.iter().any(|row| row.value["summary"]
                .as_str()
                .unwrap()
                .contains("Commit statuses: read")),
            "a second distinct gap must be recorded for the same PR"
        );
        assert_eq!(
            incidents.len(),
            2,
            "each distinct gap is recorded once, including overlapping and reordered batches"
        );
        assert!(incidents.iter().all(|row| row.value["blocking"] == false));
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    } else {
        let snapshot = controller.snapshot().unwrap();
        let mut delivery: Delivery = serde_json::from_value(
            snapshot
                .rows
                .iter()
                .find(|row| {
                    row.table == "allowances"
                        && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
                })
                .unwrap()
                .value["delivery"]
                .clone(),
        )
        .unwrap();
        delivery.pull_request = Some(100);
        delivery.head_sha = Some(
            api.state.lock().unwrap().pull["head"]["sha"]
                .as_str()
                .unwrap()
                .into(),
        );
        {
            let mut state = api.state.lock().unwrap();
            state.checks = fixture("waiting", "checks");
            state.required = true;
            let head = state.pull["head"]["sha"].clone();
            state.head_checks = vec![json!({"id":901,"name":"Sandbox tests","head_sha":head,
                "status":"completed","conclusion":"success","app":{"id":15368,"slug":"github-actions"}})];
        }
        let mut adapter = api.adapter(&root.path().join("check-reads"), 0);
        let translator = Translator::new(api.clients()).unwrap();
        let mut read = || -> Vec<Observation> {
            if mode == "adapter" {
                adapter.observations(&delivery).unwrap()
            } else {
                let mut body = webhook_fixture();
                body["action"] = json!("completed");
                body["check_run"] = api.state.lock().unwrap().queue_check();
                translator
                    .webhook(&parsed("check_run", body), &snapshot, &bootstrap().repos)
                    .unwrap()
                    .into_iter()
                    .filter_map(|input| {
                        if let tl3_controller::cli::Input::Observation { observation, .. } = input {
                            Some(observation)
                        } else {
                            None
                        }
                    })
                    .collect()
            }
        };
        let before = api.state.lock().unwrap().reads;
        assert!(read().is_empty());
        let cold = api.state.lock().unwrap().reads - before;
        let before = api.state.lock().unwrap().reads;
        let before_paths = api.state.lock().unwrap().get_paths.clone();
        assert!(read().is_empty());
        let unchanged = api.state.lock().unwrap().reads - before;
        let after_paths = api.state.lock().unwrap().get_paths.clone();
        let check_path = format!(
            "/repos/owner/sandbox/commits/{}/check-runs",
            delivery.head_sha.as_deref().unwrap()
        );
        for path in [
            check_path.as_str(),
            "/repos/owner/sandbox/rules/branches/main",
            "/repos/owner/sandbox/branches/main",
        ] {
            assert_eq!(
                after_paths[path] - before_paths[path],
                1,
                "exactly one authoritative read of {path}"
            );
        }
        assert_eq!(
            unchanged, 5,
            "each queue observation must avoid the duplicate check-run read; keep two PR reads, one check-run read, and two protection reads"
        );
        println!(
            "queued {mode} REST reads: cold={cold}, unchanged={unchanged}; previous implementation=6"
        );
    }
    println!("test check_reads_{mode} ... ok");
}

fn report_gap(controller: &mut Controller, api: &Server, source: &str) {
    let snapshot = controller.snapshot().unwrap();
    if source == "look" {
        let key = &snapshot
            .rows
            .iter()
            .find(|row| {
                row.table == "allowances"
                    && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
            })
            .unwrap()
            .id;
        controller.look_key(key).unwrap();
    } else {
        let mut body = webhook_fixture();
        body["action"] = json!("completed");
        body["check_run"] = api.state.lock().unwrap().queue_check();
        for input in Translator::new(api.clients())
            .unwrap()
            .webhook(&parsed("check_run", body), &snapshot, &bootstrap().repos)
            .unwrap()
        {
            ingress::apply(controller, input).unwrap();
        }
    }
    drain(controller);
}

fn clear_gap(controller: &mut Controller, id: &str, sequence: u64) {
    controller
        .observe(
            &format!("clear-gap-{sequence}"),
            Observation::GitHubComment {
                repository: bootstrap().repos[0].clone(),
                issue: 12,
                comment: tl3_github::Comment {
                    id: 90000 + sequence,
                    body: format!("clear incident {id}"),
                    user: tl3_github::Actor {
                        id: 42,
                        login: "fixture".into(),
                    },
                    created_at: chrono::DateTime::UNIX_EPOCH,
                    updated_at: chrono::DateTime::UNIX_EPOCH,
                    issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
                },
            },
        )
        .unwrap();
    for _ in 0..100 {
        if controller.snapshot().unwrap().rows.iter().any(|row| {
            row.table == "operations_comments"
                && row.id.ends_with(&format!("/{}", 90000 + sequence))
        }) {
            break;
        }
        assert!(controller.drive_once().unwrap());
    }
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.table == "incidents" && row.id == id)
            .unwrap()
            .value["open"],
        false,
        "Rob's clear must close the incident before new evidence arrives"
    );
}

fn reopen(controller: &mut Controller, api: &Server, source: &str) {
    {
        let mut state = api.state.lock().unwrap();
        state.checks = fixture("waiting", "checks");
        state.restricted = Some(("branch".into(), 403));
    }
    report_gap(controller, api, source);
    let incident = controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.table == "incidents" && row.id.starts_with("required-checks:"))
        .unwrap();
    assert_eq!(incident.value["open"], true);
    for sequence in 1..=2 {
        clear_gap(controller, &incident.id, sequence);
        report_gap(controller, api, source);
        let snapshot = controller.snapshot().unwrap();
        let incidents: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "incidents" && row.id.starts_with("required-checks:"))
            .collect();
        assert_eq!(
            incidents.len(),
            1,
            "reopening must keep the incident's identity"
        );
        assert_eq!(incidents[0].id, incident.id);
        assert_eq!(
            incidents[0].value["open"], true,
            "a persisting permission gap must reopen after clear on the next {source}"
        );
        assert_eq!(incidents[0].value["blocking"], false);
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            if source == "look" {
                Phase::Publishing
            } else {
                Phase::Queued
            }
        );
    }
    clear_gap(controller, &incident.id, 3);
    api.state.lock().unwrap().restricted = None;
    report_gap(controller, api, source);
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.table == "incidents"
                && row.id.starts_with("required-checks:")
                && row.id == incident.id)
            .unwrap()
            .value["open"],
        false,
        "a resolved gap must stay cleared"
    );
}
