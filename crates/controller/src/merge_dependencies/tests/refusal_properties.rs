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
// A counter incident owns exactly one world arrival. Remove only that allocation
// from the high-water value, just as its row and audit receipt are removed.
const ALLOCATIONS_WITHOUT_COUNTER_INCIDENTS: &str = "SELECT jsonb_set(to_jsonb(t), '{last_id}', to_jsonb(last_id - CASE WHEN relation = 'tl3_world'::regclass AND column_name = 'arrival' THEN (SELECT count(*) FROM tl3_world WHERE table_name = 'incidents' AND value #>> '{signature,class}' = 'kernel_counter_headroom') ELSE 0 END)) FROM tl3_input_allocations t";

fn business_bytes(c: &mut Controller) -> Vec<u8> {
    let mut reader = c.store.reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let tables: Vec<String> = tx.query(
        "SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename LIKE 'tl3_%' ORDER BY tablename", &[]
    ).unwrap().into_iter().map(|r| r.get(0)).collect();
    let mut dump = BTreeMap::<String, Value>::new();
    for table in tables {
        if table == "tl3_input_allocations" {
            let mut values: Vec<Value> = tx
                .query(ALLOCATIONS_WITHOUT_COUNTER_INCIDENTS, &[])
                .unwrap()
                .into_iter()
                .map(|row| row.get(0))
                .collect();
            values.sort_by_cached_key(Value::to_string);
            dump.insert(table, json!(values));
            continue;
        }
        if table == "tl3_meta" {
            // The only additional audit receipt is the separate incident commit.
            dump.insert(table, tx.query_one("SELECT jsonb_build_array(jsonb_build_object('singleton', singleton, 'revision', revision - (SELECT count(*) FROM tl3_events WHERE payload ? 'kernel_refusal'), 'epoch', epoch)) FROM tl3_meta", &[]).unwrap().get(0));
            continue;
        }
        let filter = if table == "tl3_events" {
            "WHERE NOT (payload ? 'kernel_refusal')"
        } else if table == "tl3_world" {
            "WHERE NOT (table_name = 'incidents' AND value #>> '{signature,class}' = 'kernel_counter_headroom')"
        } else {
            ""
        };
        let sql = format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]') FROM {table} t {filter}"
        );
        dump.insert(table, tx.query_one(&sql, &[]).unwrap().get(0));
    }
    let sequences: Vec<String> = tx.query("SELECT sequencename FROM pg_sequences WHERE schemaname='public' AND sequencename LIKE 'tl3_%' ORDER BY sequencename", &[])
        .unwrap().into_iter().map(|r| r.get(0)).collect();
    for sequence in sequences {
        dump.insert(sequence.clone(), tx.query_one(&format!("SELECT jsonb_build_object('last_value',last_value,'is_called',is_called) FROM {sequence}"), &[]).unwrap().get(0));
    }
    serde_json::to_vec(&dump).unwrap()
}

fn comment(repository: &Repository, number: u64, actor: u64, body: &str) -> Observation {
    Observation::GitHubComment {
        repository: repository.clone(),
        issue: number,
        comment: tl3_github::Comment {
            id: 7714,
            body: body.into(),
            user: tl3_github::Actor {
                id: actor,
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
        String::from_utf8(business_bytes(&mut c)).unwrap(),
        String::from_utf8(before).unwrap(),
        "queue translation must not retain its receipt or an intern when a fan-out step refuses"
    );
}

fn classified_refusal() {
    let mut db = Database::start();
    let (_repo, mut c, repository) = fixture(&mut db, "atomic_classified");
    intake(&mut c, &repository, 42);
    c.observe(
        "pause-comment",
        comment(&repository, 42, c.bootstrap.rob, "pause"),
    )
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
        String::from_utf8(business_bytes(&mut c)).unwrap(),
        String::from_utf8(before).unwrap(),
        "classifier refusal must retain its job, watch, unclassified input and deduplication state"
    );
}

fn queued_intake_edits(priorities: &[u8]) {
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
        actor: c.bootstrap.rob,
        label: "tl3".into(),
        labels: vec!["P2".into()],
    })
    .unwrap();
    for (index, priority) in priorities.iter().enumerate() {
        c.issue_changed(&crate::IssueChange {
            delivery: format!("held-edit-{index}"),
            owner: repository.owner.clone(),
            repo: repository.name.clone(),
            installation: repository.installation,
            number: 2772,
            title: format!("[P{priority}] edited title {index}"),
            labels: vec![],
            updated_at: chrono::DateTime::from_timestamp(1_800_000_000 + index as i64, 0).unwrap(),
        })
        .unwrap();
    }
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
    for _ in 0..priorities.len() + 3 {
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
        [
            tl3_kernel::Priority::P0,
            tl3_kernel::Priority::P1,
            tl3_kernel::Priority::P2,
            tl3_kernel::Priority::P3
        ][*priorities.last().unwrap() as usize],
        "a complete edit must survive before its queued intake has a request row"
    );
}

fn queued_intake_edit() {
    queued_intake_edits(&[2, 1]);
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

struct FixedClock;
impl crate::clock::Clock for FixedClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000)
    }
}

fn open_case(settings: &tl3_config::Store, repo: &Repo, bootstrap: &Bootstrap) -> Controller {
    Controller::open_with_clock(
        tl3_store::Store::open(settings).unwrap(),
        crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
        bootstrap.clone(),
        FixedClock,
    )
    .unwrap()
}

// Clone only disposable fixture databases, with their owning controller closed.
// PostgreSQL copies the real tables, indexes, constraints and allocator state.
fn copy_database(settings: &tl3_config::Store, name: &str) -> tl3_config::Store {
    assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    assert!(
        settings
            .database
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
    let mut admin = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .password("tl3-disposable-test-password")
        .dbname("postgres")
        .connect(postgres::NoTls)
        .unwrap();
    admin
        .batch_execute(&format!(
            "CREATE DATABASE {name} TEMPLATE {}",
            settings.database
        ))
        .unwrap();
    let mut copied = settings.clone();
    copied.database = name.into();
    copied
}

fn clear_injected_incident(c: &mut Controller) {
    let snapshot = c.snapshot().unwrap();
    let incidents: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["signature"]["class"] == crate::input_errors::COUNTER_HEADROOM
        })
        .collect();
    assert_eq!(incidents.len(), 1);
    let row = incidents[0];
    let mut value = row.value.clone();
    value["open"] = json!(false);
    let mut clear = Commit::new(
        "property-authorized-clear",
        json!({"property_clear":true}),
        &snapshot,
    );
    clear.rows.push(put("incidents", row.id.clone(), value));
    c.store.commit(&clear).unwrap();
}

// Replay removes only the refusal/clearance audit records and maps their revision
// offsets out of references. SQL wall-clock timestamps differ between the two
// executions; arrival numbers are compared by their persisted relative order.
// The refusal comparison itself retains all these fields byte for byte.
fn replay_bytes(c: &mut Controller, initial_revision: i64) -> Vec<u8> {
    fn revision(value: i64, excluded: &[i64]) -> i64 {
        value - excluded.iter().filter(|v| **v <= value).count() as i64
    }
    fn canonical(value: &mut Value, ids: &BTreeMap<String, String>, excluded: &[i64]) {
        match value {
            Value::String(s) => {
                if let Some(mapped) = ids.get(s) {
                    *s = mapped.clone();
                } else if let Some(rest) = s.strip_prefix("notice:")
                    && let Some((number, suffix)) = rest.split_once(':')
                    && let Ok(number) = number.parse::<i64>()
                {
                    *s = format!("notice:{}:{suffix}", revision(number, excluded));
                } else if s.starts_with("cancel:")
                    && let Some((stem, number)) = s.rsplit_once(':')
                    && let Ok(number) = number.parse::<i64>()
                {
                    *s = format!("{stem}:{}", revision(number, excluded));
                }
            }
            Value::Array(a) => {
                for value in a {
                    canonical(value, ids, excluded);
                }
            }
            Value::Object(o) => {
                for value in o.values_mut() {
                    canonical(value, ids, excluded);
                }
            }
            _ => {}
        }
    }
    let mut reader = c.store.reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let excluded: Vec<i64> = tx.query("SELECT revision FROM tl3_events WHERE payload ? 'kernel_refusal' OR payload ? 'property_clear' ORDER BY revision", &[])
        .unwrap().into_iter().map(|r| r.get(0)).collect();
    let ids: BTreeMap<String, String> = tx.query("SELECT row_id FROM tl3_world WHERE table_name = 'incidents' AND NOT (value #>> '{signature,class}' = 'kernel_counter_headroom')", &[])
        .unwrap().into_iter().filter_map(|row| {
            let id: String = row.get(0);
            let (number, suffix) = id.split_once('.')?;
            let number = number.parse::<i64>().ok()?;
            (number > initial_revision).then(|| (id.clone(), format!("{}.{suffix}", revision(number, &excluded))))
        }).collect();
    let arrivals: BTreeMap<i64, usize> = tx.query("SELECT arrival FROM tl3_world WHERE NOT (table_name = 'incidents' AND value #>> '{signature,class}' = 'kernel_counter_headroom') ORDER BY arrival", &[])
        .unwrap().into_iter().enumerate().map(|(i,r)| (r.get(0), i)).collect();
    let tables: Vec<String> = tx.query("SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename LIKE 'tl3_%' ORDER BY tablename", &[])
        .unwrap().into_iter().map(|r| r.get(0)).collect();
    let mut dump = BTreeMap::<String, Vec<Value>>::new();
    for table in tables {
        if table == "tl3_input_allocations" {
            let mut values: Vec<Value> = tx
                .query(ALLOCATIONS_WITHOUT_COUNTER_INCIDENTS, &[])
                .unwrap()
                .into_iter()
                .map(|row| row.get(0))
                .collect();
            values.sort_by_cached_key(Value::to_string);
            dump.insert(table, values);
            continue;
        }
        let filter = match table.as_str() {
            "tl3_world" => {
                "WHERE NOT (table_name = 'incidents' AND value #>> '{signature,class}' = 'kernel_counter_headroom')"
            }
            "tl3_events" => "WHERE NOT (payload ? 'kernel_refusal' OR payload ? 'property_clear')",
            _ => "",
        };
        let mut values: Vec<Value> = tx.query(&format!("SELECT to_jsonb(t) - ARRAY(SELECT column_name FROM information_schema.columns WHERE table_name = $1 AND data_type LIKE 'timestamp%') FROM {table} t {filter}"), &[&table])
            .unwrap().into_iter().map(|r| r.get(0)).collect();
        for value in &mut values {
            for column in ["revision", "created_revision", "updated_revision"] {
                if let Some(number) = value[column].as_i64() {
                    value[column] = json!(revision(number, &excluded));
                }
            }
            if table == "tl3_world" {
                value["version"] = json!(revision(value["version"].as_i64().unwrap(), &excluded));
                value["arrival"] = json!(arrivals[&value["arrival"].as_i64().unwrap()]);
            }
            canonical(value, &ids, &excluded);
        }
        values.sort_by_cached_key(Value::to_string);
        dump.insert(table, values);
    }
    serde_json::to_vec(&dump).unwrap()
}

fn observation_inventory(
    c: &mut Controller,
    repo: &Repo,
    repository: &Repository,
    payload: u16,
) -> Vec<Observation> {
    let node = NodeId(0);
    let generation = c.world().unwrap().nodes[0].generation;
    let sha = repo.head.clone();
    let key = c
        .snapshot()
        .unwrap()
        .pending_effects
        .last()
        .unwrap()
        .key
        .clone();
    let run = crate::adapters::prismabuild::CheckRun {
        repository: repo.root.path().into(),
        base_sha: tl3_worktrees::CommitId::new(&sha).unwrap(),
        reviewed_sha: None,
        reproducing_unavailable: None,
        plan: vec![],
        selected_failures: vec![],
    };
    let issue = serde_json::from_value(json!({"number":2772,"title":"replacement","body":null,"state":"open","updated_at":"2026-11-03T23:00:00Z"})).unwrap();
    vec![
        Observation::Prerequisite(ExternalFact {
            repository: repository.clone(),
            kind: PrerequisiteKind::Issue,
            number: 2761,
            status: DependencyStatus::Delivered,
        }),
        Observation::ScratchSaved {
            node,
            reference: "saved".into(),
        },
        Observation::ScratchPruned {
            reference: "saved".into(),
        },
        Observation::TraceLogsCapped { evicted: 1 },
        Observation::Progress {
            key: key.clone(),
            checkpoint: format!("checkpoint-{payload}"),
        },
        Observation::Quota {
            key: key.clone(),
            reset_ms: 1_800_000_001_000,
            model: None,
        },
        Observation::Classified {
            key: "absent-job".into(),
            reply: Some(json!({"kind":"pause"})),
        },
        Observation::Postmortem {
            key: "absent-job".into(),
            reply: None,
            failure: Some("launch_failed".into()),
            judge_failure: None,
        },
        Observation::LinkedRequest {
            repository: repository.clone(),
            issue,
            original: node,
        },
        Observation::Queue(crate::adapters::mergify::QueueFact {
            repository: repository.clone(),
            number: 7714,
            evidence: crate::adapters::mergify::Evidence::Merged {
                head: sha.clone(),
                checks_passed: true,
            },
        }),
        comment(repository, 42, c.bootstrap.rob, &format!("pause {payload}")),
        Observation::WorkerFailed {
            node,
            generation,
            failure_class: "permanent_failure".into(),
            failure_message: format!("failure-{payload}"),
        },
        Observation::WorkerReply {
            node,
            generation,
            role: "ingest".into(),
            model: None,
            reply: json!({"kind":"brief","brief":"Implement","goals":[{"description":"Implement","verification":"test"}],"memory_ids":[],"files":[],"forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true},"merge_prerequisites":[]}),
        },
        Observation::Kernel(Event::Fact {
            node,
            fact: Fact::Priority(tl3_kernel::Priority::P1),
        }),
        Observation::Steering {
            node,
            command: Command::Pause,
        },
        Observation::CiFailed {
            node,
            infra: false,
            checks: vec![],
        },
        Observation::WorktreePrepared {
            node,
            repository: repo.root.path().into(),
            base_sha: sha.clone(),
            sha: sha.clone(),
        },
        Observation::CheckPlan {
            node,
            sha: sha.clone(),
            run,
        },
        Observation::Checks {
            node,
            generation,
            action_key: "check".into(),
            base_sha: sha.clone(),
            sha: sha.clone(),
            changed_paths: vec![format!("src/file_{payload}.rs")],
            failed_tests: vec![],
            report: false,
            pass: false,
            results: vec![],
        },
        Observation::Plan {
            node,
            generation,
            paths: vec![format!("src/file_{payload}.rs")],
            fits: true,
        },
        Observation::Made {
            node,
            generation,
            sha: sha.clone(),
        },
        Observation::Reviewed {
            node,
            generation,
            sha: sha.clone(),
        },
        Observation::RebaseConflict {
            node,
            generation,
            head: sha.clone(),
            target: sha.clone(),
            files: vec![format!("src/file_{payload}.rs")],
        },
        Observation::Rebased {
            node,
            generation,
            sha: sha.clone(),
        },
        Observation::Merged {
            node,
            sha,
            checks_passed: true,
        },
        Observation::PullRequest { node, number: 7714 },
    ]
}

fn queue_case(c: &mut Controller, table: &str, value: Value) {
    let snapshot = c.snapshot().unwrap();
    let mut queued = Commit::new("property-input", value.clone(), &snapshot);
    queued.rows.push(put(table, "property-input".into(), value));
    c.store.commit(&queued).unwrap();
}

/// State-table "Counter headroom": the rollback property runs the real store,
/// translation and every fan-out step, with a restart before the retained retry.
fn check_input_case(db: &mut Database, case: usize, payload: u16, run: usize) {
    let name = format!("property_{case}_{run}");
    let (repo, mut c, repository, settings) = fixture_with_settings(db, &name);
    c.clock = Box::new(FixedClock);
    let root = intake(&mut c, &repository, 42);
    if case == 0 {
        let other = intake(&mut c, &repository, 2772);
        ingest(&mut c, root, &[2761]);
        ingest(&mut c, other, &[2761]);
    }
    if case == 9 {
        let first = intake(&mut c, &repository, 2761);
        let second = intake(&mut c, &repository, 2772);
        ingest(&mut c, first, &[42]);
        ingest(&mut c, second, &[42]);
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
        persist_world(&mut c, &world, "property-merge-ready");
    }
    if case == 31 {
        event(
            &mut c,
            Event::Command {
                node: root,
                id: 1,
                command: Command::Stop,
            },
        );
        confirm(&mut c, root);
        let issue: tl3_github::Issue = serde_json::from_value(json!({"number":42,"title":"replacement","body":"new scope","state":"open","updated_at":"2026-11-03T23:00:00Z"})).unwrap();
        let task = crate::steering::Task::Restart {
            comment_key: "restart".into(),
            text: "restart".into(),
            issue: Some(issue),
        };
        let snapshot = c.snapshot().unwrap();
        let mut pending = Commit::new("property-replacement", json!(null), &snapshot);
        pending
            .rows
            .push(put("replacement_pending", "0".into(), json!(task)));
        pending
            .rows
            .push(put("replacement", "0".into(), json!(task)));
        c.store.commit(&pending).unwrap();
    }
    if case == 32 {
        crate::adapters::github::set_fake_issue(&repo.root.path().join("fakes"), &serde_json::from_value(json!({"number":42,"title":"[P1] fresh priority","body":null,"state":"open","updated_at":"2026-11-05T23:00:00Z"})).unwrap());
    }
    let mut binding = Commit::new("property-pr", json!(null), &c.snapshot().unwrap());
    binding
        .rows
        .push(put("pull_requests", "0".into(), json!({"number":7714})));
    c.store.commit(&binding).unwrap();
    let mut observations = observation_inventory(&mut c, &repo, &repository, payload);
    let table;
    let value;
    if case < observations.len() {
        // Inventory drift fails when the accepted enum gains a variant.
        let names: BTreeSet<String> = observations
            .iter()
            .map(|o| json!(o).as_object().unwrap().keys().next().unwrap().clone())
            .collect();
        let source = include_str!("../../executor.rs")
            .split_once("pub enum Observation {")
            .unwrap()
            .1
            .split_once("\n}\n")
            .unwrap()
            .0;
        let declared: BTreeSet<String> = source
            .lines()
            .filter(|l| l.starts_with("    ") && !l.starts_with("        "))
            .map(str::trim)
            .filter(|l| l.chars().next().is_some_and(char::is_uppercase))
            .map(|l| {
                l.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(names, declared);
        if case == 6 {
            c.observe(
                "classify",
                comment(&repository, 42, c.bootstrap.rob, "pause"),
            )
            .unwrap();
            c.drive_once().unwrap();
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
            observations[case] = Observation::Classified {
                key,
                reply: Some(json!({"kind":"pause"})),
            };
        }
        let observation = observations.swap_remove(case);
        c.observe("property-input", observation).unwrap();
        let snapshot = c.snapshot().unwrap();
        let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "inbox" && r.id == "external:property-input")
        else {
            // Trace-log cap and receipt deduplication finish at admission; they
            // cannot reach a kernel step or a headroom refusal.
            assert_eq!(case, 3);
            return;
        };
        table = row.table.clone();
        value = row.value.clone();
    } else {
        use crate::input_errors::PendingKernelInput as Input;
        let input = match case - observations.len() {
            0 => Input::Intake {
                issue: LabelledIssue {
                    delivery: "property-new".into(),
                    owner: repository.owner.clone(),
                    repo: repository.name.clone(),
                    installation: repository.installation,
                    number: 2772,
                    title: format!("[P1] Request {payload}"),
                    actor: c.bootstrap.rob,
                    label: "tl3".into(),
                    labels: vec![],
                },
                proposed: None,
            },
            1 => Input::IssueChange(crate::IssueChange {
                delivery: "property-edit".into(),
                owner: repository.owner.clone(),
                repo: repository.name.clone(),
                installation: repository.installation,
                number: 42,
                title: format!("[P1] Edit {payload}"),
                labels: vec![],
                updated_at: "2026-11-05T09:01:00Z".parse().unwrap(),
            }),
            2 => Input::Event(Event::Fact {
                node: root,
                fact: Fact::Priority(tl3_kernel::Priority::P1),
            }),
            3 | 4 => {
                let snapshot = c.snapshot().unwrap();
                let effect = snapshot.pending_effects.last().unwrap();
                if case - observations.len() == 3 {
                    let mut sent = Commit::new("property-sent", json!(null), &snapshot);
                    sent.transitions.push(tl3_store::EffectTransition {
                        key: effect.key.clone(),
                        from: effect.state,
                        to: tl3_store::EffectState::Sent,
                    });
                    c.store.commit(&sent).unwrap();
                    Input::Completion {
                        key: effect.key.clone(),
                        completion: crate::executor::Completion::confirmed(vec![
                            Observation::PullRequest {
                                node: root,
                                number: 7714,
                            },
                        ]),
                    }
                } else {
                    Input::CancelAbsent {
                        key: effect.key.clone(),
                    }
                }
            }
            5 => Input::Replacement { original: root },
            6 => Input::PriorityReread {
                id: "0".into(),
                row: crate::Reconcile {
                    repository: repository.clone(),
                    number: 42,
                    updated_at: "2026-11-05T09:01:00Z".parse().unwrap(),
                    generation: 0,
                    attempts: 0,
                    next_due_ms: 0,
                },
                now: 1_800_000_000_000,
            },
            _ => unreachable!(),
        };
        table = crate::input_errors::PENDING_KERNEL.into();
        value = json!(input);
        queue_case(&mut c, &table, value.clone());
    }
    let original = c.snapshot().unwrap();
    let row = original
        .rows
        .iter()
        .find(|r| r.table == table && r.value == value)
        .unwrap()
        .clone();
    let initial_revision = original.revision.get();
    let bootstrap = c.bootstrap.clone();
    drop(c);
    let baseline_settings = copy_database(&settings, &format!("{name}_baseline"));
    let mut baseline = open_case(&baseline_settings, &repo, &bootstrap);
    baseline.refusal_injection.borrow_mut().visited = 0;
    assert!(
        baseline.consume_kernel_input(&original, &row).unwrap(),
        "case {case} must have a successful baseline"
    );
    let reached = baseline.snapshot().unwrap();
    let witness_table = match case {
        7 => Some("postmortem_jobs"),
        16 => Some("check_plans"),
        22 => Some("rebase_conflicts"),
        _ => None,
    };
    if let Some(table) = witness_table {
        let projection = |snapshot: &Snapshot| {
            snapshot
                .rows
                .iter()
                .filter(|r| r.table == table)
                .map(|r| (r.id.clone(), r.value.clone()))
                .collect::<Vec<_>>()
        };
        assert_ne!(
            projection(&original),
            projection(&reached),
            "case {case}: active handler must write {table} before refusal can qualify"
        );
    }
    let checkpoints = baseline.refusal_injection.borrow().visited;
    assert!(checkpoints > 0);
    if case == 0 || case == 9 {
        assert!(
            checkpoints >= 3,
            "fan-out fixture must execute every waiter step"
        );
    }
    let expected = replay_bytes(&mut baseline, initial_revision);
    drop(baseline);
    for checkpoint in 0..checkpoints {
        let trial_settings = copy_database(&settings, &format!("{name}_at_{checkpoint}"));
        let mut trial = open_case(&trial_settings, &repo, &bootstrap);
        let before = business_bytes(&mut trial);
        *trial.refusal_injection.borrow_mut() = crate::input_errors::RefusalInjection {
            at: Some(checkpoint),
            visited: 0,
        };
        assert!(
            !trial.consume_kernel_input(&original, &row).unwrap(),
            "case {case}, checkpoint {checkpoint}"
        );
        assert_eq!(
            String::from_utf8(business_bytes(&mut trial)).unwrap(),
            String::from_utf8(before).unwrap(),
            "refusal mutated case {case}, checkpoint {checkpoint}"
        );
        clear_injected_incident(&mut trial);
        drop(trial);
        let mut trial = open_case(&trial_settings, &repo, &bootstrap);
        assert!(
            trial.drive_once().unwrap(),
            "retained input must wake after clearance"
        );
        assert_eq!(
            String::from_utf8(replay_bytes(&mut trial, initial_revision)).unwrap(),
            String::from_utf8(expected.clone()).unwrap(),
            "replay diverged for case {case}, checkpoint {checkpoint}"
        );
    }
}

macro_rules! refusal_group {
    ($name:ident, $start:expr, $end:expr) => {
        #[test]
        fn $name() {
            let db = std::cell::RefCell::new(Database::start());
            for case in $start..$end {
                let run = std::cell::Cell::new(0);
                let mut runner = TestRunner::new(Config {
                    cases: 2,
                    max_shrink_iters: 16,
                    rng_seed: RngSeed::Fixed(771_400 + case as u64),
                    source_file: Some(file!()),
                    failure_persistence: Some(Box::new(
                        proptest::test_runner::FileFailurePersistence::Direct(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/src/merge_dependencies/tests/refusal-regressions.txt"
                        )),
                    )),
                    ..Config::default()
                });
                runner
                    .run(&any::<u16>(), |payload| {
                        run.set(run.get() + 1);
                        check_input_case(&mut db.borrow_mut(), case, payload, run.get());
                        Ok(())
                    })
                    .unwrap();
            }
        }
    };
}
refusal_group!(refusal_observations_0, 0, 4);
refusal_group!(refusal_observations_1, 4, 8);
refusal_group!(refusal_observations_2, 8, 12);
refusal_group!(refusal_observations_3, 12, 16);
refusal_group!(refusal_observations_4, 16, 20);
refusal_group!(refusal_observations_5, 20, 24);
refusal_group!(refusal_direct_inputs, 24, 33);

/// State-table "Counter headroom" / Q39: a queued intake followed by complete
/// edits retains the final priority through refusal, restart and ordered replay.
#[test]
fn halted_intake_edit_sequences() {
    let mut runner = TestRunner::new(Config {
        cases: 8,
        rng_seed: RngSeed::Fixed(771_403),
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/merge_dependencies/tests/refusal-regressions.txt"
            )),
        )),
        ..Config::default()
    });
    runner
        .run(&proptest::collection::vec(0u8..4, 1..7), |priorities| {
            queued_intake_edits(&priorities);
            Ok(())
        })
        .unwrap();
}

/// State-table "Counter headroom": a filesystem wait subscription survives
/// refusal. Its deletion is retried from committed intent after incident repair.
#[test]
fn wait_subscription_is_removed_only_after_input_commit() {
    use crate::executor::{Completion, Delivery, Executor, Reconciliation, Upstream};
    use std::{cell::Cell, rc::Rc};
    struct Waits {
        path: std::path::PathBuf,
        calls: Rc<Cell<usize>>,
        fail: Rc<Cell<bool>>,
    }
    impl Executor for Waits {
        fn execute(&mut self, _: &Delivery) -> crate::Result<Completion> {
            unreachable!()
        }
        fn reconcile(&mut self, _: &Delivery) -> crate::Result<Reconciliation> {
            unreachable!()
        }
        fn watch_wait(
            &mut self,
            _: &Delivery,
            wait: Option<&tl3_store::ExternalWait>,
        ) -> crate::Result<()> {
            assert!(wait.is_none());
            self.calls.set(self.calls.get() + 1);
            if self.fail.get() {
                return Err(Error::Invalid("subscription unavailable"));
            }
            std::fs::remove_file(&self.path)?;
            Ok(())
        }
    }
    let mut db = Database::start();
    let (repo, mut c, repository) = fixture(&mut db, "atomic_wait");
    intake(&mut c, &repository, 42);
    let snapshot = c.snapshot().unwrap();
    let allowance = snapshot
        .rows
        .iter()
        .find(|r| r.table == "allowances")
        .unwrap();
    let key = allowance.id.clone();
    let mut value = allowance.value.clone();
    value["external_wait"] = json!({"upstream":"prismabuild","key":"pending-check"});
    let mut seed = Commit::new("seed-subscription", json!(null), &snapshot);
    seed.rows.push(put("allowances", key.clone(), value));
    c.store.commit(&seed).unwrap();
    let path = repo.root.path().join("subscription");
    std::fs::write(&path, "pending-check").unwrap();
    let calls = Rc::new(Cell::new(0));
    let fail = Rc::new(Cell::new(true));
    c.adapters.register(
        Upstream::PrismaBuild,
        Waits {
            path: path.clone(),
            calls: calls.clone(),
            fail: fail.clone(),
        },
    );
    c.observe(
        "resumed",
        Observation::Progress {
            key,
            checkpoint: "own-work".into(),
        },
    )
    .unwrap();
    let snapshot = c.snapshot().unwrap();
    let row = snapshot.rows.iter().find(|r| r.table == "inbox").unwrap();
    let before = business_bytes(&mut c);
    *c.refusal_injection.borrow_mut() = crate::input_errors::RefusalInjection {
        at: Some(0),
        visited: 0,
    };
    assert!(!c.consume_kernel_input(&snapshot, row).unwrap());
    assert_eq!(calls.get(), 0);
    assert!(path.is_file());
    assert_eq!(business_bytes(&mut c), before);
    clear_injected_incident(&mut c);
    *c.refusal_injection.borrow_mut() = Default::default();
    let snapshot = c.snapshot().unwrap();
    let row = snapshot.rows.iter().find(|r| r.table == "inbox").unwrap();
    assert!(c.consume_kernel_input(&snapshot, row).unwrap());
    assert_eq!(
        calls.get(),
        0,
        "SQL publication precedes filesystem removal"
    );
    let snapshot = c.snapshot().unwrap();
    assert!(c.retire_committed_waits(&snapshot).unwrap());
    assert_eq!(calls.get(), 1);
    let snapshot = c.snapshot().unwrap();
    assert!(
        !c.retire_committed_waits(&snapshot).unwrap(),
        "an open incident prevents polling"
    );
    assert_eq!(calls.get(), 1);
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "retiring_waits")
        .unwrap();
    let incident = row.value["incident"].as_str().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "incidents" && r.id == incident)
        .unwrap();
    let mut value = row.value.clone();
    value["open"] = json!(false);
    let mut clear = Commit::new("repair-subscription", json!(null), &snapshot);
    clear.rows.push(put("incidents", row.id.clone(), value));
    c.store.commit(&clear).unwrap();
    fail.set(false);
    let snapshot = c.snapshot().unwrap();
    assert!(c.retire_committed_waits(&snapshot).unwrap());
    assert_eq!(calls.get(), 2);
    assert!(!path.exists());
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "retiring_waits")
    );
}

/// State-table "Counter headroom": postmortem refusal must reach its active job.
#[test]
fn handler_reachability_postmortem() {
    check_input_case(&mut Database::start(), 7, 771, 1);
}

/// State-table "Counter headroom": adoption worktree evidence must write its plan.
#[test]
fn handler_reachability_worktree() {
    check_input_case(&mut Database::start(), 16, 771, 1);
}

/// State-table "Counter headroom": conflict refusal must reach Rebasing writes.
#[test]
fn handler_reachability_rebase_conflict() {
    check_input_case(&mut Database::start(), 22, 771, 1);
}

/// State-table "Counter headroom": a slow upstream read must not stop ingress,
/// memory-feed publication or supervisor progress on their independent sessions.
#[test]
fn slow_input_http_does_not_block_other_writers() {
    use crate::executor::{Completion, Delivery, Executor, Reconciliation, Upstream};
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        time::{Duration, Instant},
    };
    struct HttpPrerequisite(std::net::SocketAddr);
    impl Executor for HttpPrerequisite {
        fn prerequisite(
            &mut self,
            _: &Repository,
            _: PrerequisiteKind,
            _: u64,
        ) -> crate::Result<DependencyStatus> {
            let mut socket = TcpStream::connect(self.0).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .write_all(
                    b"GET /prerequisite HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200"));
            Ok(DependencyStatus::Pending)
        }
        fn execute(&mut self, _: &Delivery) -> crate::Result<Completion> {
            unreachable!()
        }
        fn reconcile(&mut self, _: &Delivery) -> crate::Result<Reconciliation> {
            unreachable!()
        }
    }
    let mut db = Database::start();
    let (repo, mut c, _, settings) = fixture_with_settings(&mut db, "http_transaction_writers");
    let repository = c.bootstrap.repos[0].clone();
    let node = intake(&mut c, &repository, 42);
    c.store
        .put_worker_materials(
            "0",
            &json!({"issue":{"title":"Implement","body":"Wait for #9999"}}),
        )
        .unwrap();
    let generation = c.world().unwrap().nodes[0].generation;
    c.observe("http-input", Observation::WorkerReply {
        node, generation, role: "ingest".into(), model: None,
        reply: json!({"kind":"brief","brief":"Implement","goals":[{"description":"Implement","verification":"test"}],"memory_ids":[],"files":["src/blocked.rs"],"forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"src/blocked.rs":1},"overhead_tokens":0,"tokens":2,"fit_tokens":100,"context_tokens":150,"fits":true},"merge_prerequisites":[{"kind":"issue","number":9999,"source_quote":"Wait for #9999"}]}),
    }).unwrap();
    let bootstrap = c.bootstrap.clone();
    let epoch = c.snapshot().unwrap().epoch.get();
    let fake_root = repo.root.path().join("fakes");
    let mut memory = tl3_store::Store::open(&settings).unwrap();
    let mut worker = tl3_store::Store::open(&settings).unwrap();
    let mut ingress = tl3_store::Store::open(&settings).unwrap();
    let mut sql = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .password("tl3-disposable-test-password")
        .dbname(&settings.database)
        .connect(postgres::NoTls)
        .unwrap();
    drop(c);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        assert!(socket.read(&mut request).unwrap() > 0);
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });
    let input_settings = settings.clone();
    let input = std::thread::spawn(move || {
        let mut adapters = crate::adapters::simulation(&fake_root).unwrap();
        adapters.register(Upstream::GitHub, HttpPrerequisite(address));
        let mut c = Controller::open_with_clock(
            tl3_store::Store::open(&input_settings).unwrap(),
            adapters,
            bootstrap,
            FixedClock,
        )
        .unwrap();
        let original = c.snapshot().unwrap();
        let row = original
            .rows
            .iter()
            .find(|r| r.table == "inbox" && r.id == "external:http-input")
            .unwrap()
            .clone();
        assert!(c.consume_kernel_input(&original, &row).unwrap());
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let locks: Vec<Value> = sql.query("SELECT jsonb_build_object('type', l.locktype, 'table', c.relname, 'mode', l.mode, 'granted', l.granted) FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid LEFT JOIN pg_class c ON c.oid=l.relation WHERE a.datname=current_database() AND a.state='idle in transaction' ORDER BY l.locktype,c.relname,l.mode", &[]).unwrap().into_iter().map(|r| r.get(0)).collect();
    println!("INPUT_HTTP_LOCKS {}", json!(locks));
    let (done_tx, done_rx) = mpsc::channel();
    let tx = done_tx.clone();
    let memory_thread = std::thread::spawn(move || {
        let start = Instant::now();
        memory
            .queue_memory("http-independent-memory", &json!({"summary":"independent"}))
            .unwrap();
        tx.send(("memory_feed", start.elapsed().as_millis()))
            .unwrap();
    });
    let tx = done_tx.clone();
    let worker_thread = std::thread::spawn(move || {
        let start = Instant::now();
        worker.claim_worker("http-worker", "0", epoch, 1).unwrap();
        worker
            .record_worker_progress("http-worker", 1, 1, "tool_result", None)
            .unwrap();
        tx.send(("worker_supervisor", start.elapsed().as_millis()))
            .unwrap();
    });
    let ingress_thread = std::thread::spawn(move || {
        let start = Instant::now();
        assert!(
            ingress
                .receive_ingress(
                    "github-webhook",
                    "http-independent-ingress",
                    "issues",
                    b"{}"
                )
                .unwrap()
        );
        sql.execute("INSERT INTO tl3_world (table_name,row_id,value,version) SELECT 'inbox','http-independent-inbox','{}'::jsonb,revision FROM tl3_meta", &[]).unwrap();
        done_tx
            .send(("ingress_and_inbox", start.elapsed().as_millis()))
            .unwrap();
    });
    let deadline = Instant::now() + Duration::from_millis(750);
    let mut results = Vec::new();
    while results.len() < 3 {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok(result) = done_rx.recv_timeout(remaining) else {
            break;
        };
        results.push(result);
    }
    release_tx.send(()).unwrap();
    server.join().unwrap();
    input.join().unwrap();
    memory_thread.join().unwrap();
    worker_thread.join().unwrap();
    ingress_thread.join().unwrap();
    println!("INPUT_HTTP_WRITERS {}", json!(results));
    assert_eq!(
        results.len(),
        3,
        "all independent writers must commit while HTTP is still blocked"
    );
    assert!(results.iter().all(|(_, millis)| *millis < 750));
}
