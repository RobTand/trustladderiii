use super::*;

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    // This webhook was emitted before the merge, but processed after it.
    let mut stale = pull_event(
        &api,
        if mode == "closed" {
            "closed"
        } else {
            "unlabeled"
        },
    );
    stale["label"] = json!({"name":if mode == "queue-label" { "tl3-queue" } else { "dequeued" }});
    if mode == "closed" {
        stale["pull_request"]["state"] = json!("closed");
    }
    assert!(stale["pull_request"]["merged_at"].is_null());
    {
        let mut state = api.state.lock().unwrap();
        state.merged(false);
        state.checks = fixture(
            if mode == "pending" {
                "running"
            } else {
                "merged"
            },
            "checks",
        );
        // merge_bot_account is a person login, so neither event is filtered as a bot.
        for (id, event) in [(500, "merged"), (501, "closed")] {
            state.events.push(
                json!({"id":id,"event":event,"actor":{"id":42,"login":"rob"},
                "created_at":"2026-10-30T01:00:00Z","issue":{"number":100}}),
            );
        }
        if mode == "queue-label" {
            state.events.push(json!({"id":502,"event":"unlabeled","actor":{"id":42,"login":"rob"},
                "label":{"name":"tl3-queue"},"created_at":"2026-10-30T01:00:01Z","issue":{"number":100}}));
        }
    }
    let inputs = Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed("pull_request", stale.clone()),
            &controller.snapshot().unwrap(),
            &bootstrap().repos,
        )
        .unwrap();
    assert!(
        inputs.iter().all(|input| {
            !matches!(input, tl3_controller::cli::Input::Observation {
            observation: tl3_controller::executor::Observation::Queue(fact), ..
        } if matches!(fact.evidence,
            tl3_controller::adapters::mergify::Evidence::ClosedByPerson { .. }
            | tl3_controller::adapters::mergify::Evidence::DequeuedByPerson { .. }))
        }),
        "a merge's person-close event must never become a stop or pause"
    );
    for input in inputs {
        ingress::apply(&mut controller, input).unwrap();
    }
    drain(&mut controller);
    if mode == "pending" {
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Queued,
            "wait for the final merge check without stopping"
        );
        api.state.lock().unwrap().checks = fixture("merged", "checks");
    } else {
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Closed(Outcome::Merged),
            "fresh PR state must complete the request before the merge webhook arrives"
        );
    }
    // The real merge webhook and stale redelivery must be harmless, including after restart.
    drop(controller);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.path().join("mergify"), 0),
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    deliver(settings, &api, "pull_request", &stale);
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    let effects = Store::open(settings).unwrap().effects().unwrap();
    assert!(
        effects
            .iter()
            .any(|effect| { effect.payload["output"]["Notice"]["notice"]["Summary"] == "Merged" }),
        "send the normal merged completion summary"
    );
    for effect in effects {
        let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
        assert!(
            !matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::ClosePr,
                    ..
                } | Out::Notice {
                    notice: tl3_kernel::Notice::Summary(Outcome::Stopped),
                    ..
                } | Out::Notice {
                    notice: tl3_kernel::Notice::Note(
                        tl3_kernel::NoteKind::PrClosed
                            | tl3_kernel::NoteKind::PrDequeued
                            | tl3_kernel::NoteKind::LateMerge
                    ),
                    ..
                }
            ),
            "no stop notice or PR teardown after merge: {:?}",
            delivery.output
        );
    }
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| { row.table == "incidents" && row.value["open"] == true }),
        "an ordinary approved merge must leave no open incident"
    );
    assert_eq!(api.state.lock().unwrap().deletes, 0);
    println!("test merge_closure_{mode} ... ok");
}
