use super::*;

pub(super) struct Fixture;
impl Fixture {
    pub(super) fn respond(&self, path: &str) -> Option<ResponseTemplate> {
        let body: Value = if path.ends_with("/pulls/3331") {
            serde_json::from_str(include_str!("../fixtures/mergify/pq3221/draft.json")).unwrap()
        } else if path.contains("/commits/4a3d6c618043e1875de665093edfe83f87f255ea/") {
            serde_json::from_str(include_str!("../fixtures/mergify/pq3221/draft-checks.json")).unwrap()
        } else if path.ends_with("/pulls/100/files") {
            json!([{"filename":"src/change.rs"}])
        } else if path.contains("/actions/jobs/") && path.ends_with("/logs") {
            return Some(ResponseTemplate::new(200).set_body_string(
                "plan refused at planning; upload refused\nProcess completed with exit code 75."
            ));
        } else { return None; };
        Some(ResponseTemplate::new(200).set_body_json(body))
    }
}

/// #854b / Q50: real PQ #3221 queue refusal must hold without a failed round.
pub(super) fn scenario(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut config = bootstrap();
    config.fail_fast = true;
    let mut controller = queued_for_clock(settings, &api, root.path(), true, &[], config.clone(), None);
    // PQ's failed fix round had fail_fast enabled; preserve that decision state.
    let snapshot = controller.snapshot().unwrap();
    let row = snapshot.rows.iter().find(|row| row.table == "nodes" && row.id == "0").unwrap();
    let mut value = row.value.clone();
    value["round"] = json!(1);
    let mut seed = tl3_store::Commit::new("pq-fix-round", json!({}), &snapshot);
    seed.rows.push(tl3_store::RowChange::Put { table: "nodes".into(), id: "0".into(), value });
    Store::open(settings).unwrap().commit(&seed).unwrap();
    let before = controller.world().unwrap().nodes[0];
    {
        let mut state = api.state.lock().unwrap();
        let head = state.pull["head"]["sha"].clone();
        let mut captured: Value = serde_json::from_str(include_str!("../fixtures/mergify/pq3221/checks.json")).unwrap();
        for check in captured["check_runs"].as_array_mut().unwrap() {
            check["head_sha"] = head.clone();
            check["pull_requests"] = json!([]);
        }
        state.checks = captured["check_runs"].clone();
        state.queue_rule["conclusion"] = json!("success");
        state.pull["base"]["sha"] = json!("ef817b9d48f4e2f55a3b7d8e83caef25573ad544");
        state.cause_fixture = Some(Fixture);
    }
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    for input in Translator::new(api.clients()).unwrap().webhook(
        &parsed("check_run", body), &controller.snapshot().unwrap(), &config.repos,
    ).unwrap() { ingress::apply(&mut controller, input).unwrap(); }
    for _ in 0..12 { controller.drive_once().unwrap(); }
    let after = controller.world().unwrap().nodes[0];
    assert_eq!(after.round, before.round, "opaque PQ refusal must hold, never consume a fix round");
    assert_eq!(after.credit.spent, before.credit.spent);
    assert_eq!(after.phase, Phase::Queued, "hold keeps the PR open without fail_fast");
    assert_eq!(api.state.lock().unwrap().requeues, 0);
    assert!(controller.snapshot().unwrap().rows.iter().any(|row| row.table == "queue_holds"));
    println!("test queue_cause_pq_unattributed ... ok");
}
