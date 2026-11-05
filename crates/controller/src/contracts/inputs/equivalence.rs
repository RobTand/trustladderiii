//! Real old/new builders share a database snapshot and a recorded Hindsight edge.
use super::*;
use crate::git_tests::{Repo, support::Database};
use std::ops::Deref;

struct Reference<'a>(&'a Supervisor);
impl Deref for Reference<'_> {
    type Target = Supervisor;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

mod review {
    use super::*;
    use crate::adapters::workers::review::criteria;
    pub(super) use crate::adapters::workers::review::{resumes, session};
    include!("reference/review_before.rs");
}
include!("reference/workers_before.rs");

/// #730 C1/C2, #658/#663/#717: the pre-migration builder remains executable.
/// Each generated schedule changes the facts between requests, then both builders
/// consume those same facts. No worker is launched by this differential test.
#[test]
fn old_and_typed_request_corpus_is_byte_identical() {
    use proptest::{prelude::*, test_runner::TestRunner};
    let mut db = Database::start();
    let repo = Repo::new();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("typed_inputs_parity");
    config.workers.runs = repo.root.path().join("runs");
    config.worktrees_root = repo.root.path().join("worktrees");
    config.hindsight.endpoint = "http://127.0.0.1:9".parse().unwrap();
    config.hindsight.api_key_file = None;
    let registry = crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap();
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "typed-inputs".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Fix greeting".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    let supervisor = Supervisor::new(
        &config,
        &repo.root.path().join("records"),
        &mut crate::store_support::ownership(),
    )
    .unwrap();
    // Warm the degraded recall edge. Both builders then receive the same retained
    // breaker response, rather than making a second network attempt.
    let mut store = Store::open(&config.store).unwrap();
    crate::memory::recall(
        &mut store,
        &config.hindsight,
        &supervisor.recall_breaker,
        crate::memory::Recall::Context {
            repo: "RobTand/tl3-sandbox",
            query: "parity",
        },
    )
    .unwrap();

    let before = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("parity-postmortem", Value::Null, &before);
    commit.postmortems.push(tl3_store::OwedPostmortem {
        node: "0".into(),
        trigger: "rob:parity".into(),
        reason: "rob_asked".into(),
        sample_share: None,
        repo: "RobTand/tl3-sandbox".into(),
        issue: 42,
        title: "Fix greeting".into(),
    });
    store.commit(&commit).unwrap();
    let id = store.owed_postmortem().unwrap().unwrap().id;
    let snapshot = store.load().unwrap();
    let delivery = repo.delivery();
    let old = crate::postmortems::input_reference::case_file(
        &mut store,
        &snapshot,
        &delivery,
        id,
        &config.models,
        config.limits.postmortem_deadline_secs,
    )
    .unwrap();
    let new = crate::postmortems::case_file(
        &mut store,
        &snapshot,
        &delivery,
        id,
        &config.models,
        config.limits.postmortem_deadline_secs,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&new).unwrap(),
        serde_json::to_vec(&old).unwrap()
    );
    let strategy = ("[a-zA-Z0-9é]{1,48}", 1u8..=3, any::<bool>(), any::<bool>());
    TestRunner::new(ProptestConfig::with_cases(16)).run(&strategy, |(text, attempt, finding, session)| {
            let mut delivery = repo.delivery();
            let mut store = Store::open(&config.store).unwrap();
        delivery.attempt = attempt;
        let brief = json!({"kind":"brief", "memory_ids":[], "brief":text,
            "goals":[{"description":text,"verification":"review"}]});
        let audit = json!({"verdict":"disagree", "objections":[text], "evidence_contract":[]});
        let mut snapshot = schedule_documents(&mut store, &[("briefs",brief), ("audits",audit), ("findings",json!([text])),
            ("maker_findings",if finding { json!([{"kind":"failed_check","report":text,"goal_ids":["g1"]}]) } else { json!([]) }),
            ("goals", json!([{"id":"g1","description":text}])),
            ("review_fixes",json!({"head_sha":repo.head,"reviewed_sha":repo.base,
                "findings":[{"id":"f1","report":text,"reason":"No valid assertion proof"}]}))]);
        snapshot.rows.push(tl3_store::WorldRow { table:"check_reports".into(), id:format!("0:{}",repo.head),
            value:json!({"base_sha":repo.base,"selected_failures":[],"results":[{
                "name":"regressions","kind":"regression","verdict":"pass","custody":"in-process",
                "tests":[{"test":"test_a.py","base":"passed","head":"passed","pre_existing":false,"passed":true}]
            }]}) });
        let materials = json!({
            "base_checkout":{"repository":repo.path,"commit":repo.base},
            "issue":{"number":42,"title":text,"body":text,"author":"requester","comments":[]},
            "repo":{"readme":text,"file_list":["README.md"]},
            "maker":{"brief":text,"goals":[{"id":"g1","description":text}],
                "plan":{"files":["README.md"],"checks":[{"name":"regressions","kind":"regression",
                    "tests":["test_a.py"],"goal_ids":["g1"]}]},
                "feedback":[{"kind":"failed_check","report":text,"goal_ids":["g1"]}]}
        });
        store.put_worker_materials("0", &materials).unwrap();
        for role in ["ingest", "audit", "planner", "maker", "reviewer", "classifier"] {
            delivery.steering = if role == "classifier" { Some(crate::steering::Task::Classify {
                comment_key: "parity".into(), inputs: json!({"comment":{"author":"requester","body":text},
                    "state":{"phase":"Planning","closed":false,"restartable":true,"open_question":null}})
            }) } else { None };
            // Session choices only affect the maker; old/new must sample the same
            // checkpoint existence and store row, even when the row changes.
            let file = config.workers.runs.join("sessions/0/plan-0").join(format!("attempt-{attempt}.json"));
            if session { fs::create_dir_all(file.parent().unwrap()).unwrap(); fs::write(&file, "{}").unwrap(); }
            else if file.exists() { fs::remove_file(&file).unwrap(); }
            let old = Reference(&supervisor).build_request(&delivery, role, &mut store, &snapshot).unwrap();
            let new = supervisor.build_request(&delivery, role, &mut store, &snapshot).unwrap();
            prop_assert_eq!(serde_json::to_vec(&new).unwrap(), serde_json::to_vec(&old).unwrap(), "{}", role);

        }
        Ok(())
    }).unwrap();
}

fn schedule_documents(store: &mut Store, values: &[(&str, Value)]) -> Snapshot {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let before = store.load().unwrap();
    let key = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut commit = tl3_store::Commit::new(format!("input-schedule-{key}"), Value::Null, &before);
    commit.rows = values
        .iter()
        .map(|(table, v)| crate::put(table, "0".into(), v.clone()))
        .collect();
    store.commit(&commit).unwrap();
    store.load().unwrap()
}
