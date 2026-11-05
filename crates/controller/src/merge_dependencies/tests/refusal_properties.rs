//! State-table "Counter headroom": refusal is a store-wide no-op and replay
//! must match an uninterrupted input, including the real translation side tables.
use super::*;
use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed, TestRunner},
};

fn persist_world(c: &mut Controller, world: &World, key: &str) {
    let snapshot = c.snapshot().unwrap();
    let mut commit = Commit::new(key, json!(null), &snapshot);
    for ((table, id), value) in crate::snapshot::encode(world).unwrap() {
        commit.rows.push(put(&table, id, value));
    }
    c.store.commit(&commit).unwrap();
}

// Query every persisted table through the actual reader. Incident bookkeeping is
// the sole allowed change. In particular, this includes interned IDs, outbox,
// credit, worker records, queue receipts, jobs, watches and request/edit rows.
fn business_bytes(c: &mut Controller) -> Vec<u8> {
    let mut reader = c.store.reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let tables: Vec<String> = tx.query(
        "SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename LIKE 'tl3_%' ORDER BY tablename", &[]
    ).unwrap().into_iter().map(|r| r.get(0)).collect();
    let mut dump = BTreeMap::<String, Value>::new();
    for table in tables {
        if table == "tl3_meta" || table == "tl3_events" {
            continue;
        }
        let filter = if table == "tl3_world" {
            "WHERE NOT (table_name = 'incidents' AND value #>> '{signature,class}' = 'kernel_counter_headroom')"
        } else {
            ""
        };
        let sql = format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]') FROM {table} t {filter}"
        );
        dump.insert(table, tx.query_one(&sql, &[]).unwrap().get(0));
    }
    serde_json::to_vec(&dump).unwrap()
}

fn comment(repository: &Repository, number: u64, body: &str) -> Observation {
    Observation::GitHubComment {
        repository: repository.clone(),
        issue: number,
        comment: tl3_github::Comment {
            id: 7714,
            body: body.into(),
            user: tl3_github::Actor {
                id: 42,
                login: "rob".into(),
            },
            created_at: "2026-11-05T09:00:00Z".parse().unwrap(),
            updated_at: "2026-11-05T09:00:00Z".parse().unwrap(),
            issue_url: format!(
                "https://api.github.com/repos/{}/{}/issues/{number}",
                repository.owner, repository.name
            ),
        },
    }
}

fn queue_merge_refusal() {
    let mut db = Database::start();
    let (repo, mut c, repository) = fixture(&mut db, "atomic_queue_merge");
    let root = intake(&mut c, &repository, 2761);
    let first = intake(&mut c, &repository, 42);
    let second = intake(&mut c, &repository, 2772);
    ingest(&mut c, first, &[2761]);
    ingest(&mut c, second, &[2761]);
    event(
        &mut c,
        Event::Command {
            node: root,
            id: 1,
            command: Command::Stop,
        },
    );
    confirm(&mut c, root);
    let mut world = c.world().unwrap();
    world.nodes[0].had_pr = true;
    world.nodes[0].credit.ready = true;
    world.nodes[0].credit.start = 4;
    world.next_ticket = u64::MAX - world.ticket_step_budget().unwrap();
    persist_world(&mut c, &world, "seed-queue-refusal");
    let snapshot = c.snapshot().unwrap();
    let mut binding = Commit::new("bind-queue-pr", json!(null), &snapshot);
    binding
        .rows
        .push(put("pull_requests", "0".into(), json!({"number":7714})));
    c.store.commit(&binding).unwrap();
    c.observe(
        "queue-merge-refusal",
        Observation::Queue(crate::adapters::mergify::QueueFact {
            repository,
            number: 7714,
            evidence: crate::adapters::mergify::Evidence::Merged {
                head: repo.head,
                checks_passed: true,
            },
        }),
    )
    .unwrap();
    let before = business_bytes(&mut c);
    assert!(c.drive_once().unwrap());
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents"
                && r.value["signature"]["class"] == "kernel_counter_headroom")
    );
    assert_eq!(
        business_bytes(&mut c),
        before,
        "queue translation must not retain its receipt or an intern when a fan-out step refuses"
    );
}

fn classified_refusal() {
    let mut db = Database::start();
    let (_repo, mut c, repository) = fixture(&mut db, "atomic_classified");
    intake(&mut c, &repository, 42);
    c.observe("pause-comment", comment(&repository, 42, "pause"))
        .unwrap();
    assert!(c.drive_once().unwrap());
    let snapshot = c.snapshot().unwrap();
    assert!(c.schedule_comment(&snapshot).unwrap());
    let key = c
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .find(|r| r.table == "comment_jobs")
        .unwrap()
        .id
        .clone();
    let mut world = c.world().unwrap();
    world.nodes[0].last_cmd = u64::MAX;
    persist_world(&mut c, &world, "seed-command-refusal");
    c.observe(
        "classified-refusal",
        Observation::Classified {
            key,
            reply: Some(json!({"kind":"pause"})),
        },
    )
    .unwrap();
    let before = business_bytes(&mut c);
    assert!(c.drive_once().unwrap());
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents"
                && r.value["signature"]["class"] == "kernel_counter_headroom")
    );
    assert_eq!(
        business_bytes(&mut c),
        before,
        "classifier refusal must retain its job, watch, unclassified input and deduplication state"
    );
}

fn queued_intake_edit() {
    let mut db = Database::start();
    let (repo, mut c, repository, settings) = fixture_with_settings(&mut db, "atomic_intake_edit");
    intake(&mut c, &repository, 42);
    let mut world = c.world().unwrap();
    let seq = world.nodes[0].next_seq;
    world.nodes[0].next_seq = u64::MAX;
    persist_world(&mut c, &world, "seed-intake-halt");
    c.intake(LabelledIssue {
        delivery: "held-intake".into(),
        owner: repository.owner.clone(),
        repo: repository.name.clone(),
        installation: repository.installation,
        number: 2772,
        title: "old title".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec!["P2".into()],
    })
    .unwrap();
    c.issue_changed(&crate::IssueChange {
        delivery: "held-edit".into(),
        owner: repository.owner.clone(),
        repo: repository.name.clone(),
        installation: repository.installation,
        number: 2772,
        title: "edited title".into(),
        labels: vec!["P1".into()],
        updated_at: "2026-11-05T09:01:00Z".parse().unwrap(),
    })
    .unwrap();
    let bootstrap = c.bootstrap.clone();
    drop(c);
    let mut c = Controller::open(
        tl3_store::Store::open(&settings).unwrap(),
        crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
        bootstrap,
    )
    .unwrap();
    let mut repaired = c.world().unwrap();
    repaired.nodes[0].next_seq = seq;
    persist_world(&mut c, &repaired, "repair-intake");
    let snapshot = c.snapshot().unwrap();
    let id = snapshot
        .rows
        .iter()
        .find(|r| r.table == "incidents" && r.value["open"] == true)
        .unwrap()
        .id
        .clone();
    let mut clear = Commit::new("test-authorized-clear", json!(null), &snapshot);
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "incidents" && r.id == id)
        .unwrap();
    let mut value = row.value.clone();
    value["open"] = json!(false);
    clear.rows.push(put("incidents", id, value));
    c.store.commit(&clear).unwrap();
    for _ in 0..4 {
        if c.world().unwrap().nodes.len() == 2
            && !c
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "pending_kernel_events")
        {
            break;
        }
        c.drive_once().unwrap();
    }
    assert_eq!(
        c.world().unwrap().nodes[1].priority,
        tl3_kernel::Priority::P1,
        "a complete edit must survive before its queued intake has a request row"
    );
}

#[test]
fn refused_queue_merge_is_a_store_noop() {
    queue_merge_refusal();
}
#[test]
fn refused_classification_is_a_store_noop() {
    classified_refusal();
}
#[test]
fn halted_intake_edit_survives_restart() {
    queued_intake_edit();
}

#[test]
fn refusal_is_a_noop_seeded() {
    let mut runner = TestRunner::new(Config {
        cases: 6,
        rng_seed: RngSeed::Fixed(7714),
        ..Config::default()
    });
    runner
        .run(&(0u8..3), |case| {
            match case {
                0 => queue_merge_refusal(),
                1 => classified_refusal(),
                _ => queued_intake_edit(),
            }
            Ok(())
        })
        .unwrap();
}
