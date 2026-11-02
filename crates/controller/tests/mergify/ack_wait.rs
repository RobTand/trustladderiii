//! #356: an enqueue Mergify hasn't acknowledged yet is a wait, not a failed read.
use super::*;

fn enqueue_allowance(controller: &mut Controller) -> (String, Delivery) {
    let snapshot = controller.snapshot().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "allowances"
                && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
        })
        .expect("the enqueue has an allowance");
    (
        row.id.clone(),
        serde_json::from_value(row.value["delivery"].clone()).unwrap(),
    )
}

fn incidents(controller: &mut Controller) -> Vec<Value> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| row.table == "incidents")
        .map(|row| row.value)
        .collect()
}

pub(super) fn scenario(settings: &tl3_config::Store) {
    let api = Server::new();
    api.state.lock().unwrap().lag = true;
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), false);
    let (key, delivery) = enqueue_allowance(&mut controller);
    // Mergify answers a queue command with a burst of webhooks, and each one looks.
    for _ in 0..4 {
        controller.look_key(&key).unwrap();
        drain(&mut controller);
    }
    assert_eq!(
        incidents(&mut controller),
        Vec::<Value>::new(),
        "waiting for Mergify's acknowledgement is not a failed read"
    );
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing
    );
    // Mergify acknowledges after the burst.
    {
        let mut state = api.state.lock().unwrap();
        state.lag = false;
        state.checks = fixture("running", "checks");
    }
    controller.look_key(&key).unwrap();
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    assert!(incidents(&mut controller).is_empty());
    // A real read error is still a failed look, not a wait.
    api.state.lock().unwrap().fail_reads = true;
    let mut adapter = api.adapter(&root.path().join("mergify"), 0);
    assert!(
        adapter.look(&delivery).is_err(),
        "a Mergify read error must reach the look as an error"
    );
    api.state.lock().unwrap().fail_reads = false;
    println!("test unacknowledged_enqueue_is_a_wait ... ok");
}
