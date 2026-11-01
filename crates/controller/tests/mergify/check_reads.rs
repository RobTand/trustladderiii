use super::*;
use tl3_controller::adapters::mergify::{Evidence, QueueFact};
use tl3_controller::executor::Observation;

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    if mode == "gaps" {
        for (index, gaps) in [
            vec!["Contents: read unavailable"],
            vec!["Commit statuses: read unavailable"],
            vec!["Contents: read unavailable", "Commit statuses: read unavailable"],
            vec!["Commit statuses: read unavailable", "Contents: read unavailable"],
        ].into_iter().enumerate() {
            controller.observe(&format!("gap-{index}"), Observation::Queue(QueueFact {
                repository: bootstrap().repos[0].clone(),
                number: 100,
                evidence: Evidence::ChecksUnavailable {
                    permissions: gaps.into_iter().map(str::to_owned).collect(),
                },
            })).unwrap();
            drain(&mut controller);
        }
        let snapshot = controller.snapshot().unwrap();
        let incidents: Vec<_> = snapshot.rows.iter().filter(|row| row.table == "incidents").collect();
        assert!(incidents.iter().any(|row| row.value["summary"].as_str().unwrap().contains("Commit statuses: read")), "a second distinct gap must be recorded for the same PR");
        assert_eq!(incidents.len(), 2, "each distinct gap is recorded once, including overlapping and reordered batches");
        assert!(incidents.iter().all(|row| row.value["blocking"] == false));
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    } else {
        let snapshot = controller.snapshot().unwrap();
        let mut delivery: Delivery = serde_json::from_value(snapshot.rows.iter().find(|row| {
            row.table == "allowances" && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
        }).unwrap().value["delivery"].clone()).unwrap();
        delivery.pull_request = Some(100);
        delivery.head_sha = Some(api.state.lock().unwrap().pull["head"]["sha"].as_str().unwrap().into());
        {
            let mut state = api.state.lock().unwrap();
            state.checks = fixture("waiting", "checks");
            state.required = true;
            let head = state.pull["head"]["sha"].clone();
            state.head_checks = vec![json!({"id":901,"name":"Sandbox tests","head_sha":head,
                "status":"completed","conclusion":"success","app":{"id":15368,"slug":"github-actions"}})];
        }
        let mut adapter = api.adapter(&root.path().join("read-cache"), 0);
        let translator = Translator::new(api.clients()).unwrap();
        let mut read = || -> Vec<Observation> {
            if mode == "adapter" {
                adapter.observations(&delivery).unwrap()
            } else {
                let mut body = webhook_fixture();
                body["action"] = json!("completed");
                body["check_run"] = api.state.lock().unwrap().queue_check();
                translator.webhook(&parsed("check_run", body), &snapshot, &bootstrap().repos).unwrap().into_iter().filter_map(|input| {
                    if let tl3_controller::cli::Input::Observation { observation, .. } = input { Some(observation) } else { None }
                }).collect()
            }
        };
        let before = api.state.lock().unwrap().reads;
        assert!(read().is_empty());
        let cold = api.state.lock().unwrap().reads - before;
        let before = api.state.lock().unwrap().reads;
        assert!(read().is_empty());
        let warm = api.state.lock().unwrap().reads - before;
        assert_eq!(warm, 5, "unchanged queue evidence must reuse the head-check read; keep two PR reads, one check-run read, and two protection reads");
        println!("queued {mode} REST reads: cold={cold}, unchanged={warm}; previous implementation=6");

        // A rerun can fail on the same SHA without changing the queue check or PR.
        api.state.lock().unwrap().head_checks[0]["conclusion"] = json!("failure");
        assert!(read().iter().any(|observation| matches!(observation, Observation::Queue(QueueFact { evidence: Evidence::CiFailed { head_check: true, infra: false, .. }, .. }))), "changed required checks must invalidate the cached success");
        // Branch protection can change without a PR or check-run update.
        api.state.lock().unwrap().required = false;
        assert!(read().is_empty(), "requirements must remain fresh");
        {
            let mut state = api.state.lock().unwrap();
            state.head_checks.clear();
            state.classic = true;
            state.classic_context_only = true;
            state.statuses = json!([{"id":50,"context":"Sandbox tests","state":"success"}]);
        }
        assert!(read().is_empty());
        api.state.lock().unwrap().statuses = json!([{"id":51,"context":"Sandbox tests","state":"failure"}]);
        assert!(read().iter().any(|observation| matches!(observation, Observation::Queue(QueueFact { evidence: Evidence::CiFailed { head_check: true, .. }, .. }))), "commit statuses must stay fresh without a status webhook");
    }
    println!("test check_reads_{mode} ... ok");
}
