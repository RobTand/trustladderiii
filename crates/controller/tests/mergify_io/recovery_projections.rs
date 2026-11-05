//! B1 r1: compare real snapshot parsing and the formerly shimmed recovery bodies.
//! Private crate paths are rebound to this fixture; source pins permit no other edits.
use super::{Delivery, Evidence, Out, Repository, RequiredCheck};
use ::std::{cell::RefCell, collections::BTreeMap};
use ::tl3_store::{Epoch, InternKind, Interned, Revision, Snapshot, WorldRow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_controller::{Error, Result, adoption};
use tl3_github::{Actor, IssueActivity};
use tl3_kernel::{EffectKind, FileId, HeadId, Node, NodeId, Phase, RepoId, World};
use tl3_mergify::{QUEUE_LABEL, decide::facts};

include!("projection-support.rs");
#[path = "current-projections.rs"]
mod current;
#[path = "reference-projections.rs"]
mod reference;

thread_local! { static SAMPLES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
fn sample(operation: &str, args: impl std::fmt::Debug) {
    SAMPLES.with(|samples| samples.borrow_mut().push(format!("{operation}:{args:?}")));
}
mod snapshot {
    use super::*;
    pub(super) fn decode(snapshot: &Snapshot) -> Result<World> {
        sample("snapshot.decode", (snapshot.revision, snapshot.epoch));
        raw_snapshot_decode(snapshot)
    }
}
fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    sample("request", node);
    raw_request(snapshot, node)
}
fn approved_head(snapshot: &Snapshot, approved: Option<HeadId>) -> Result<String> {
    sample("approved_head", approved);
    raw_approved_head(snapshot, approved)
}
fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    sample("decode_stored", (std::any::type_name::<T>(), &value));
    raw_decode_stored(value)
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Entry {
    Repair,
    Hold,
}
#[derive(Debug, PartialEq, Eq)]
struct Trace {
    samples: Vec<String>,
    result: std::result::Result<bool, String>,
}
fn run(new: bool, entry: Entry, snapshot: Option<&Snapshot>, events: &[IssueActivity]) -> Trace {
    SAMPLES.with(|samples| samples.borrow_mut().clear());
    let delivery = delivery();
    let result = if new {
        current::run(entry, &delivery, snapshot, events)
    } else {
        reference::run(entry, &delivery, snapshot, events)
    };
    Trace {
        samples: SAMPLES.with(|samples| samples.borrow_mut().drain(..).collect()),
        result: result.map_err(|error| format!("{error:?}")),
    }
}
fn compare(entry: Entry, snapshot: Option<&Snapshot>, events: &[IssueActivity]) -> Trace {
    let old = run(false, entry, snapshot, events);
    let new = run(true, entry, snapshot, events);
    assert_eq!(old, new, "recovery projection differential: {entry:?}");
    old
}
fn delivery() -> Delivery {
    serde_json::from_value(json!({"key":"enqueue-1","epoch":1,
        "output":{"Effect":{"node":0,"seq":0,"kind":"Enqueue","head":null}},
        "generation":0,"head_sha":"head", "repository":repo(),"issue":1,
        "pull_request":100,"paths":[],"attempt":0,"round":0}))
    .unwrap()
}
fn repo() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}
fn row(table: &str, id: &str, value: Value, version: i64) -> WorldRow {
    WorldRow {
        table: table.into(),
        id: id.into(),
        value,
        version: Revision::try_from(version).unwrap(),
        arrival: 0,
    }
}
fn stored_fact(evidence: Value) -> QueueFact {
    serde_json::from_value(json!({"repository":repo(),"number":100,"evidence":evidence})).unwrap()
}
fn baseline() -> Snapshot {
    let mut world = World::new(&[1], 1);
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Queued;
    node.holds = true;
    node.footprint = world.intern_footprint([FileId(0)]);
    node.attempt = 1;
    node.credit.ready = true;
    node.head = Some(HeadId(0));
    node.checked = node.head;
    node.approved = node.head;
    node.pr_open = true;
    node.had_pr = true;
    world.nodes.push(node);
    let Value::Object(mut fields) = serde_json::to_value(&world).unwrap() else {
        unreachable!()
    };
    let mut rows = Vec::new();
    for table in ["nodes", "gates", "file_sets"] {
        let Value::Array(values) = fields.remove(table).unwrap() else {
            unreachable!()
        };
        for (index, value) in values.into_iter().enumerate() {
            rows.push(row(table, &index.to_string(), value, 1));
        }
    }
    rows.push(row("kernel", "meta", json!({"format":1,"world":fields}), 1));
    rows.push(row(
        "requests",
        "0",
        json!({"repo":repo(),"issue":1,"issue_id":1}),
        1,
    ));
    rows.push(row("pull_requests", "0", json!({"number":100}), 1));
    let fact = stored_fact(json!({"Conflict":{"check":7,"head":"prior"}}));
    rows.push(row(
        "queue_facts",
        &fact.identity(),
        serde_json::to_value(fact).unwrap(),
        2,
    ));
    Snapshot {
        revision: Revision::ZERO,
        epoch: Epoch::ZERO,
        rows,
        pending_effects: vec![],
        credit: vec![],
        interned: vec![Interned {
            id: 0,
            kind: InternKind::Commit,
            scope: "0".into(),
            value: "head".into(),
        }],
    }
}
fn value<'a>(snapshot: &'a mut Snapshot, table: &str) -> &'a mut Value {
    &mut snapshot
        .rows
        .iter_mut()
        .find(|row| row.table == table)
        .unwrap()
        .value
}
fn events(human: bool) -> Vec<IssueActivity> {
    vec![serde_json::from_value(json!({"id":9,"event":"unlabeled", "created_at":"2026-11-05T00:00:00Z",
        "actor":{"id":7,"login":if human {"person"} else {"app[bot]"}},"label":{"name":QUEUE_LABEL}})).unwrap()]
}
fn held_snapshot(observed: bool, resume: Option<i64>) -> Snapshot {
    let mut snapshot = baseline();
    snapshot.rows.retain(|row| row.table != "queue_facts");
    if observed {
        let fact = stored_fact(json!({"DequeuedByPerson":{"event":9}}));
        snapshot.rows.push(row(
            "queue_facts",
            &fact.identity(),
            serde_json::to_value(fact).unwrap(),
            2,
        ));
    }
    if let Some(version) = resume {
        snapshot
            .rows
            .push(row("queue_resumes", "0", json!({}), version));
    }
    snapshot
}

// Every copied executable body is compared against its actual source. Only a
// crate-root namespace and the raw helper name change to avoid the old shims.
fn item<'a>(source: &'a str, needle: &str) -> &'a str {
    let at = source
        .find(needle)
        .unwrap_or_else(|| panic!("missing source {needle}"));
    let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let body = source[start..].find('{').unwrap() + start;
    let mut depth = 1;
    for (offset, byte) in source[body + 1..].bytes().enumerate() {
        depth += i32::from(byte == b'{') - i32::from(byte == b'}');
        if depth == 0 {
            return &source[start..body + offset + 2];
        }
    }
    panic!("unclosed source {needle}")
}
#[test]
fn production_projection_functions_are_source_pinned() {
    let production = include_str!("../../src/adapters/mergify/waiting_requeue.rs");
    let frozen = include_str!("base-waiting.rs.txt");
    for (source, copy) in [
        (production, include_str!("current-projections.rs")),
        (frozen, include_str!("reference-projections.rs")),
    ] {
        for name in [
            "repaired_head",
            "human_hold_in_events",
            "unresolved_human_hold",
        ] {
            let needle = format!("fn {name}(");
            assert_eq!(
                item(source, &needle).replace("crate::", "super::"),
                item(copy, &needle),
                "projection source drift: {name}"
            );
        }
    }
    let adapter = include_str!("../../src/adapters/mergify.rs");
    let controller = include_str!("../../src/lib.rs");
    let decoder = include_str!("../../src/snapshot.rs");
    let copy = include_str!("projection-support.rs");
    for (source, original, compiled) in [
        (decoder, "decode(", "raw_snapshot_decode("),
        (controller, "request(", "raw_request("),
        (controller, "decode_stored<", "raw_decode_stored<"),
        (adapter, "approved_head(", "raw_approved_head("),
        (adapter, "node(", "node("),
        (adapter, "identity(", "identity("),
        (adapter, "unresolved_hold(", "unresolved_hold("),
        (adapter, "heal_phase(", "heal_phase("),
        (adapter, "enqueue(", "enqueue("),
        (adapter, "actor(", "actor("),
        (adapter, "event_kind(", "event_kind("),
        (adapter, "person_facts(", "person_facts("),
    ] {
        let expected = item(source, &format!("fn {original}"))
            .replace("crate::", "self::")
            .replacen(&format!("fn {original}"), &format!("fn {compiled}"), 1);
        assert_eq!(
            expected,
            item(copy, &format!("fn {compiled}")),
            "helper source drift: {original}"
        );
    }
    for (source, name) in [(controller, "Request"), (adapter, "QueueFact")] {
        assert_eq!(
            item(source, &format!("struct {name} {{")),
            item(copy, &format!("struct {name} {{"))
        );
    }
}

/// #447/#608: decode/project errors must propagate at the same gate, without later reads.
#[test]
fn mandatory_projection_parse_errors() {
    let mut cases = Vec::new();
    let mut s = baseline();
    s.rows.retain(|r| r.table != "kernel");
    cases.push(s);
    let mut s = baseline();
    value(&mut s, "kernel")["format"] = json!(2);
    cases.push(s);
    let mut s = baseline();
    s.rows.iter_mut().find(|r| r.table == "nodes").unwrap().id = "bad".into();
    cases.push(s);
    let mut s = baseline();
    s.rows.iter_mut().find(|r| r.table == "nodes").unwrap().id = "1".into();
    cases.push(s);
    let mut s = baseline();
    s.rows.push(s.rows[0].clone());
    cases.push(s);
    let mut s = baseline();
    value(&mut s, "nodes")["paused"] = json!("bad");
    cases.push(s);
    let mut s = baseline();
    s.interned.clear();
    cases.push(s);
    let mut s = baseline();
    s.rows.retain(|r| r.table != "requests");
    cases.push(s);
    for field in ["repo", "issue", "issue_id", "proposed"] {
        let mut s = baseline();
        value(&mut s, "requests")[field] = json!(false);
        cases.push(s);
    }
    let mut s = baseline();
    *value(&mut s, "queue_facts") = json!({});
    cases.push(s);
    for s in cases {
        let trace = compare(Entry::Repair, Some(&s), &[]);
        assert!(
            trace.result.is_err(),
            "projection error not reached: {trace:?}"
        );
    }
    let mut s = held_snapshot(true, None);
    s.rows
        .iter_mut()
        .find(|r| r.table == "pull_requests")
        .unwrap()
        .id = "bad".into();
    assert!(
        compare(Entry::Hold, Some(&s), &events(true))
            .result
            .unwrap_err()
            .contains("invalid PR binding")
    );
    for table in ["requests", "queue_facts"] {
        let mut s = held_snapshot(true, None);
        *value(&mut s, table) = json!({});
        let fresh = if table == "requests" {
            events(true)
        } else {
            vec![]
        };
        assert!(compare(Entry::Hold, Some(&s), &fresh).result.is_err());
    }
    let mut s = held_snapshot(true, None);
    *value(&mut s, "queue_facts") = json!({});
    assert_eq!(
        compare(Entry::Hold, Some(&s), &events(true)).result,
        Ok(true)
    );
}

/// #447: real repair gates short-circuit before approved-head/request/history projections.
#[test]
fn mandatory_repair_projection_gates() {
    assert_eq!(compare(Entry::Repair, None, &[]).result, Ok(false));
    let baseline = baseline();
    assert_eq!(
        compare(Entry::Repair, Some(&baseline), &[]).result,
        Ok(true)
    );
    let mut s = baseline.clone();
    value(&mut s, "nodes")["paused"] = json!(true);
    s.interned.clear(); // This error must remain unreachable behind the pause.
    let trace = compare(Entry::Repair, Some(&s), &[]);
    assert_eq!(trace.result, Ok(false));
    assert!(
        !trace
            .samples
            .iter()
            .any(|call| call.starts_with("approved_head:"))
    );
    let mut s = baseline.clone();
    value(&mut s, "nodes")["phase"] = json!("Publishing");
    value(&mut s, "nodes")["pos"] = json!(3);
    value(&mut s, "nodes")["next_seq"] = json!(1);
    value(&mut s, "nodes")["effect"] = json!({"seq":0,"kind":"Enqueue","cancelling":false});
    assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(true));
    let mut s = baseline.clone();
    value(&mut s, "nodes")["phase"] = json!("Checking");
    value(&mut s, "nodes")["pos"] = json!(1);
    value(&mut s, "nodes")["job"] = json!(0);
    assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
    let mut s = baseline.clone();
    value(&mut s, "nodes")["phase"] = json!("Publishing");
    value(&mut s, "nodes")["pos"] = json!(0);
    value(&mut s, "nodes")["next_seq"] = json!(1);
    value(&mut s, "nodes")["effect"] = json!({"seq":0,"kind":"Push","cancelling":false});
    value(&mut s, "nodes")["pr_open"] = json!(false);
    assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
    let mut s = baseline.clone();
    s.interned[0].value = "other".into();
    *value(&mut s, "requests") = json!({});
    let trace = compare(Entry::Repair, Some(&s), &[]);
    assert_eq!(trace.result, Ok(false));
    assert!(
        !trace
            .samples
            .iter()
            .any(|call| call.starts_with("request:"))
    );
    let mut s = baseline.clone();
    value(&mut s, "requests")["repo"]["name"] = json!("other");
    *value(&mut s, "queue_facts") = json!({});
    assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
    for field in ["id", "number"] {
        let mut s = baseline.clone();
        if field == "id" {
            s.rows
                .iter_mut()
                .find(|r| r.table == "pull_requests")
                .unwrap()
                .id = "1".into();
        } else {
            value(&mut s, "pull_requests")["number"] = json!(101);
        }
        *value(&mut s, "queue_facts") = json!({});
        assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
    }
    // These inconsistent kernel states are rejected by the real decoder before
    // shell guards. The full-domain Verus contract separately checks their gates.
    for (field, replacement) in [
        ("waiting_incident", json!(true)),
        ("head", json!(1)),
        ("checked", json!(1)),
        ("approved", json!(null)),
    ] {
        let mut s = baseline.clone();
        value(&mut s, "nodes")[field] = replacement;
        assert!(
            compare(Entry::Repair, Some(&s), &[])
                .result
                .unwrap_err()
                .contains("invalid stored kernel world")
        );
    }
    let mut s = baseline.clone();
    s.rows.retain(|r| r.table != "nodes");
    assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
}

/// #447/#608: only same repo/PR queue CI or conflict on a different head establishes repair.
#[test]
fn mandatory_prior_head_history() {
    for (evidence, expected) in [
        (json!({"Conflict":{"check":7,"head":"prior"}}), true),
        (json!({"Conflict":{"check":7,"head":"head"}}), false),
        (
            json!({"CiFailed":{"check":7,"head":"prior","infra":false,"head_check":false}}),
            true,
        ),
        (
            json!({"CiFailed":{"check":7,"head":"head","infra":false,"head_check":false}}),
            false,
        ),
        (
            json!({"CiFailed":{"check":7,"head":"prior","infra":false,"head_check":true}}),
            false,
        ),
        (
            json!({"Merged":{"head":"prior","checks_passed":true}}),
            false,
        ),
    ] {
        let mut s = baseline();
        *value(&mut s, "queue_facts") = serde_json::to_value(stored_fact(evidence)).unwrap();
        assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(expected));
    }
    for field in ["repository", "number"] {
        let mut s = baseline();
        if field == "repository" {
            value(&mut s, "queue_facts")[field]["installation"] = json!(4);
        } else {
            value(&mut s, "queue_facts")[field] = json!(101);
        }
        assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
    }
    let mut s = baseline();
    s.rows.retain(|r| r.table != "queue_facts");
    assert_eq!(compare(Entry::Repair, Some(&s), &[]).result, Ok(false));
}

/// #608/Q41: explicit newer resumes release observed holds; missing proof keeps the hold.
#[test]
fn mandatory_snapshot_and_resume_holds() {
    assert_eq!(compare(Entry::Hold, None, &events(true)).result, Ok(true));
    assert_eq!(compare(Entry::Hold, None, &events(false)).result, Ok(false));
    for (observed, resume, held) in [
        (false, None, true),
        (false, Some(3), true),
        (true, None, true),
        (true, Some(1), true),
        (true, Some(2), true),
        (true, Some(3), false),
    ] {
        let s = held_snapshot(observed, resume);
        assert_eq!(
            compare(Entry::Hold, Some(&s), &events(true)).result,
            Ok(held)
        );
    }
    for (resume, held) in [(None, true), (Some(2), true), (Some(3), false)] {
        let s = held_snapshot(true, resume);
        assert_eq!(compare(Entry::Hold, Some(&s), &[]).result, Ok(held));
    }
    let mut s = held_snapshot(true, Some(3));
    s.rows.retain(|r| r.table != "pull_requests");
    assert_eq!(
        compare(Entry::Hold, Some(&s), &events(true)).result,
        Ok(true)
    );
    let mut s = held_snapshot(true, Some(3));
    value(&mut s, "requests")["repo"]["name"] = json!("other");
    assert_eq!(
        compare(Entry::Hold, Some(&s), &events(true)).result,
        Ok(true)
    );
    let s = held_snapshot(false, None);
    assert_eq!(
        compare(Entry::Hold, Some(&s), &events(false)).result,
        Ok(false)
    );
}

#[test]
fn projection_schedules() {
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x0075_1760_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/mergify_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(
            &(
                any::<bool>(),
                any::<bool>(),
                0i64..5,
                any::<bool>(),
                any::<bool>(),
            ),
            |(observed, human, resume, paused, malformed)| {
                let mut s = held_snapshot(observed, Some(resume));
                value(&mut s, "nodes")["paused"] = json!(paused);
                if malformed {
                    value(&mut s, "requests")["issue_id"] = json!("bad");
                }
                compare(Entry::Repair, Some(&s), &[]);
                compare(Entry::Hold, Some(&s), &events(human));
                Ok(())
            },
        )
        .unwrap();
}
