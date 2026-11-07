use crate::{Error, Result, decode_stored, put};
use serde_json::Value;
use tl3_kernel::{Event, NodeId, PlannerResult, ReplanBudget};
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Copy, Debug)]
pub(crate) enum KernelInput {
    Event(Event),
    Planner { node: NodeId, generation: u64, result: PlannerResult },
}

impl From<Event> for KernelInput {
    fn from(event: Event) -> Self { Self::Event(event) }
}

/// Pending deletion is authoritative, just like a pending replacement row.
pub(crate) fn request_row(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Value> {
    let id = node.0.to_string();
    for change in commit.rows.iter().rev() {
        match change {
            RowChange::Put { table, id: key, value } if table == "requests" && *key == id => {
                return Ok(value.clone());
            }
            RowChange::Delete { table, id: key } if table == "requests" && *key == id => {
                return Err(Error::Invalid("deleted request binding"));
            }
            _ => {}
        }
    }
    snapshot.rows.iter().find(|row| row.table == "requests" && row.id == id)
        .map(|row| row.value.clone()).ok_or(Error::Invalid("missing request binding"))
}

pub(crate) fn load_budget(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<ReplanBudget> {
    let row = request_row(snapshot, commit, node)?;
    if row.get("replan_budget_format").is_some_and(|format| format != 1) {
        return Err(Error::Invalid("unsupported replan budget format"));
    }
    let legacy = snapshot.rows.iter().find(|row| row.table == "nodes" && row.id == node.0.to_string())
        .and_then(|row| row.value.get("empty_allowed_replanned"));
    let consumed = match legacy {
        Some(Value::Bool(consumed)) => *consumed,
        Some(_) => return Err(Error::Invalid("invalid legacy replan budget")),
        None => false,
    };
    let budget = match row.get("replan_budget") {
        Some(value) => decode_stored::<ReplanBudget>(value.clone())?,
        None => {
            // Only unversioned legacy rows and genuinely new request rows may
            // lack a budget. A pending overwrite cannot erase persisted state.
            if row.get("replan_budget_format").is_some() || snapshot.rows.iter().any(|old|
                old.table == "requests" && old.id == node.0.to_string()
                    && (old.value.get("replan_budget").is_some() || old.value.get("replan_budget_format").is_some())) {
                return Err(Error::Invalid("removed request replan budget"));
            }
            // Inspect raw legacy storage before World decoding drops the old field.
            ReplanBudget::import(node, consumed)
        }
    };
    if !budget.bound_to(node) { return Err(Error::Invalid("replan budget binding mismatch")); }
    if let Some(old) = snapshot.rows.iter().find(|row| row.table == "requests" && row.id == node.0.to_string()) {
        if let Some(value) = old.value.get("replan_budget") {
            let persisted = decode_stored::<ReplanBudget>(value.clone())?;
            if !persisted.bound_to(node) || (persisted.used() && !budget.used() && !consumed) {
                return Err(Error::Invalid("replan budget reset"));
            }
        }
    }
    Ok(if consumed { ReplanBudget::import(node, true) } else { budget })
}

pub(crate) fn save_budget(snapshot: &Snapshot, commit: &mut Commit, node: NodeId, budget: ReplanBudget) -> Result<()> {
    if !budget.bound_to(node) { return Err(Error::Invalid("replan budget binding mismatch")); }
    if let Some(old) = snapshot.rows.iter().find(|row| row.table == "requests" && row.id == node.0.to_string()) {
        if let Some(value) = old.value.get("replan_budget") {
            let persisted = decode_stored::<ReplanBudget>(value.clone())?;
            if !persisted.bound_to(node) || (persisted.used() && !budget.used()) {
                return Err(Error::Invalid("replan budget reset"));
            }
        }
    }
    let mut row = request_row(snapshot, commit, node)?;
    let fields = row.as_object_mut().ok_or(Error::Invalid("invalid request binding"))?;
    let encoded = serde_json::to_value(budget)?;
    if fields.get("replan_budget") == Some(&encoded)
        && fields.get("replan_budget_format") == Some(&serde_json::json!(1)) {
        return Ok(());
    }
    fields.insert("replan_budget".into(), encoded);
    fields.insert("replan_budget_format".into(), serde_json::json!(1));
    commit.rows.push(put("requests", node.0.to_string(), row));
    Ok(())
}

#[cfg(test)]
#[path = "../../store/tests/support/mod.rs"]
mod support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bootstrap, Controller, LabelledIssue, Repository, adapters, snapshot};
    use serde_json::json;
    use tl3_kernel::*;
    use tl3_store::Store;

    fn bootstrap(fail_fast: bool) -> Bootstrap {
        Bootstrap { operations: None, repos: vec![Repository { owner: "owner".into(), name: "repo".into(), installation: 1 }], rob: 1, slots: 1, pool: 1, fail_fast }
    }

    fn confirm(world: &mut World) {
        for _ in 0..32 {
            let Some(effect) = world.nodes[0].effect else { return };
            *world = step(world.clone(), Event::Ack { node: NodeId(0), seq: effect.seq, ack: Ack::Confirmed }).0;
        }
        panic!("fixture did not settle effects");
    }

    fn planning(fail_fast: bool) -> World {
        let mut w = step(World::new(&[1], 1), Event::NewRequest { repo: RepoId(0), priority: Priority::Unranked }).0;
        w.fail_fast = fail_fast;
        confirm(&mut w);
        let generation = w.nodes[0].generation;
        w = step(w, Event::Reply { node: NodeId(0), generation, reply: Reply::IngestDone { size: 2 } }).0;
        confirm(&mut w);
        let generation = w.nodes[0].generation;
        w = step(w, Event::Reply { node: NodeId(0), generation, reply: Reply::AuditAgrees }).0;
        confirm(&mut w);
        assert!(matches!(w.nodes[0].phase, Phase::Planning { .. }));
        w.check().unwrap();
        assert!(w.is_settled());
        w
    }

    fn seed(c: &mut Controller, world: World, legacy: bool) {
        c.intake(LabelledIssue { delivery: "seed".into(), owner: "owner".into(), repo: "repo".into(), installation: 1, number: 930, title: "Budget".into(), actor: 1, label: "tl3".into(), labels: vec![] }).unwrap();
        let snapshot = c.store.load().unwrap();
        let mut commit = Commit::new("seed-planning", json!({}), &snapshot);
        commit.rows.extend(snapshot::changes(&snapshot::encode(&snapshot::decode(&snapshot).unwrap()).unwrap(), snapshot::encode(&world).unwrap()));
        let mut binding = request_row(&snapshot, &commit, NodeId(0)).unwrap();
        binding.as_object_mut().unwrap().remove("replan_budget");
        binding.as_object_mut().unwrap().remove("replan_budget_format");
        binding["extra_metadata"] = json!({"retain": [1, 2, 3]});
        commit.rows.push(put("requests", "0".into(), binding));
        if legacy {
            let mut raw = serde_json::to_value(world.nodes[0]).unwrap();
            raw["empty_allowed_replanned"] = json!(true);
            commit.rows.push(put("nodes", "0".into(), raw));
        }
        c.store.commit(&commit).unwrap();
    }

    fn prepare(c: &mut Controller, snapshot: &Snapshot, key: &str, generation: u64, result: PlannerResult) -> Commit {
        let mut commit = Commit::new(key, json!({"planner":key}), snapshot);
        c.apply_input(snapshot, snapshot::decode(snapshot).unwrap(), KernelInput::Planner { node: NodeId(0), generation, result }, &mut commit).unwrap();
        commit
    }

    #[test]
    fn planner_budget_real_store_conflict_reopen_duplicate_and_new_version() {
        let mut database = support::Database::start();
        for fail_fast in [false, true] {
            let settings = database.create(&format!("rp_budget_{fail_fast}"));
            let root = tempfile::tempdir().unwrap();
            let mut c = Controller::open(Store::open(&settings).unwrap(), adapters::simulation(root.path()).unwrap(), bootstrap(fail_fast)).unwrap();
            seed(&mut c, planning(fail_fast), false);
            let snapshot = c.store.load().unwrap();
            let generation = snapshot::decode(&snapshot).unwrap().nodes[0].generation;
            let cases: Vec<Value> = serde_json::from_str(include_str!("../tests/fixtures/pq-2648-empty-replan.json")).unwrap();
            let mut queue = Commit::new("race-input", json!({}), &snapshot);
            queue.rows.push(put("inbox", "race-delivery".into(), json!({"WorkerReply": {
                "node":0,"generation":generation,"role":"planner","reply":cases[0]["reply"],"model":null,
                "plan_check":{"objections":[{"allowed_empty":true,"class":"other","rule":"proof.omitted","items":["proof"],"diagnostic":"missing proof"}],"repair":{"status":"attempted"}}
            }})));
            c.store.commit(&queue).unwrap();
            let start = c.store.load().unwrap();
            let before = snapshot::decode(&start).unwrap();
            let generation = before.nodes[0].generation;
            let first = prepare(&mut c, &start, "first", generation, PlannerResult::EmptyAllowedReplan);
            let losing = prepare(&mut c, &start, "losing", generation, PlannerResult::EmptyAllowedReplan);
            c.store.commit(&first).unwrap();
            assert!(matches!(c.store.commit(&losing), Err(tl3_store::Error::Stale { .. })));
            let row = start.rows.iter().find(|r| r.table == "inbox" && r.id == "race-delivery").unwrap();
            assert!(!c.consume_kernel_input(&start, row).unwrap());
            let fresh = c.store.load().unwrap();
            let row = fresh.rows.iter().find(|r| r.table == "inbox" && r.id == "race-delivery").unwrap();
            assert!(c.consume_kernel_input(&fresh, row).unwrap());
            let persisted = c.store.load().unwrap();
            let budget = load_budget(&persisted, &Commit::new("read", json!({}), &persisted), NodeId(0)).unwrap();
            assert!(budget.used());
            let request = persisted.rows.iter().find(|r| r.table == "requests" && r.id == "0").unwrap();
            let node = persisted.rows.iter().find(|r| r.table == "nodes" && r.id == "0").unwrap();
            assert_eq!(request.version, node.version);
            assert_eq!(request.value["extra_metadata"], json!({"retain": [1, 2, 3]}));
            let journal: crate::replay::PlannerStepRecord = serde_json::from_value(first.result["planner_replay"].clone()).unwrap();
            journal.check().unwrap();
            assert_eq!(journal.after_budget, budget);
            let duplicate = prepare(&mut c, &persisted, "duplicate", generation, PlannerResult::EmptyAllowedReplan);
            c.store.commit(&duplicate).unwrap();
            assert_eq!(snapshot::decode(&c.store.load().unwrap()).unwrap(), journal.after);
            drop(c);
            let mut c = Controller::open(Store::open(&settings).unwrap(), adapters::simulation(root.path()).unwrap(), bootstrap(fail_fast)).unwrap();
            let snapshot = c.store.load().unwrap();
            let mut after = snapshot::decode(&snapshot).unwrap();
            confirm(&mut after);
            let mut ack = Commit::new("confirm-replan", json!({}), &snapshot);
            ack.rows.extend(snapshot::changes(&snapshot::encode(&snapshot::decode(&snapshot).unwrap()).unwrap(), snapshot::encode(&after).unwrap()));
            c.store.commit(&ack).unwrap();
            let snapshot = c.store.load().unwrap();
            let repeated = prepare(&mut c, &snapshot, "repeat", after.nodes[0].generation, PlannerResult::EmptyAllowedReplan);
            c.store.commit(&repeated).unwrap();
            let snapshot = c.store.load().unwrap();
            let stopped = snapshot::decode(&snapshot).unwrap();
            assert!(matches!(stopped.nodes[0].phase, Phase::Stopping(_) | Phase::Closed(_)));
            assert_eq!(stopped.nodes[0].review_stop, Some(ReviewStop::PlanCantMeet));
            assert_eq!(stopped.nodes[0].credit, before.nodes[0].credit);
            assert!(stopped.nodes[0].question.is_none());
            let mut reader = tl3_store::Reader::open(&settings).unwrap();
            let report = crate::replay::run(&mut reader, Some("repeat")).unwrap();
            assert_eq!(report.steps, 1);
            let mut tampered = Commit::new("tamper-budget", json!({}), &snapshot);
            let mut binding = request_row(&snapshot, &tampered, NodeId(0)).unwrap();
            binding["replan_budget"] = json!(ReplanBudget::new(NodeId(0)));
            tampered.rows.push(put("requests", "0".into(), binding));
            c.store.commit(&tampered).unwrap();
            assert!(crate::replay::run(&mut reader, Some("repeat")).unwrap_err().to_string().contains("budget"));
            let mut restore = Commit::new("restore-budget", json!({}), &c.store.load().unwrap());
            restore.rows.push(put("requests", "0".into(), request_row(&snapshot, &restore, NodeId(0)).unwrap()));
            c.store.commit(&restore).unwrap();
            let snapshot = c.store.load().unwrap();
            let mut replacement = Commit::new("request-replacement", json!({}), &snapshot);
            let issue: tl3_github::Issue = serde_json::from_value(json!({"number":930,
                "title":"Budget replacement","body":"New request version","state":"open",
                "updated_at":"2026-11-07T00:00:00Z"})).unwrap();
            replacement.rows.push(put("replacement_pending", "0".into(), json!(crate::steering::Task::Restart {
                comment_key:"replacement-comment".into(), text:"restart".into(), issue:Some(issue),
            })));
            replacement.rows.push(put("request_decisions", "0".into(),
                serde_json::from_str(include_str!("../tests/fixtures/decisions/pq2662.json")).unwrap()));
            c.store.commit(&replacement).unwrap();
            let snapshot = c.store.load().unwrap();
            let mut version = Commit::new("new-version", json!({}), &snapshot);
            let labelled = LabelledIssue { delivery:"new-version".into(), owner:"owner".into(),
                repo:"repo".into(), installation:1, number:930, title:"Budget replacement".into(),
                actor:1, label:"tl3".into(), labels:vec![] };
            let event = c.prepare_intake(&snapshot, &labelled, bootstrap(fail_fast).repos.remove(0), None, &mut version).unwrap().unwrap();
            c.apply(&snapshot, stopped, event, &mut version).unwrap();
            c.store.commit(&version).unwrap();
            let snapshot = c.store.load().unwrap();
            let read = Commit::new("read", json!({}), &snapshot);
            assert!(load_budget(&snapshot, &read, NodeId(0)).unwrap().used());
            assert!(!load_budget(&snapshot, &read, NodeId(1)).unwrap().used());
            let inherited = crate::decisions::Context::load(&snapshot, NodeId(1)).unwrap();
            assert_eq!(inherited.request_version, 1);
            assert_eq!(inherited.decisions[0].id, "pq2662-q10");
            assert_eq!(inherited.decisions[0].origin_request_version, 0);
        }
    }

    #[test]
    fn planner_budget_legacy_import_pending_writes_and_corrupt_bindings() {
        let mut database = support::Database::start();
        let settings = database.create("rp_budget_legacy");
        let root = tempfile::tempdir().unwrap();
        let mut c = Controller::open(Store::open(&settings).unwrap(), adapters::simulation(root.path()).unwrap(), bootstrap(false)).unwrap();
        seed(&mut c, planning(false), true);
        let snapshot = c.store.load().unwrap();
        let mut commit = Commit::new("legacy-generic-rewrite", json!({}), &snapshot);
        c.apply(&snapshot, snapshot::decode(&snapshot).unwrap(), Event::Fact { node: NodeId(0), fact: Fact::Priority(Priority::P0) }, &mut commit).unwrap();
        c.store.commit(&commit).unwrap();
        let snapshot = c.store.load().unwrap();
        let mut pending = Commit::new("pending", json!({}), &snapshot);
        assert!(load_budget(&snapshot, &pending, NodeId(0)).unwrap().used());
        assert!(snapshot.rows.iter().find(|r| r.table == "nodes" && r.id == "0").unwrap().value.get("empty_allowed_replanned").is_none());
        let mut binding = request_row(&snapshot, &pending, NodeId(0)).unwrap();
        for corrupt in [
            { let mut row = binding.clone(); row.as_object_mut().unwrap().remove("replan_budget"); row },
            { let mut row = binding.clone(); row["replan_budget"] = json!(ReplanBudget::new(NodeId(0))); row },
            { let mut row = binding.clone(); row["replan_budget_format"] = json!(99); row },
        ] {
            pending.rows.push(put("requests", "0".into(), corrupt));
            assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        }
        binding["replan_budget"] = json!(ReplanBudget::new(NodeId(1)));
        pending.rows.push(put("requests", "0".into(), binding.clone()));
        assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        binding["replan_budget"] = json!({"request":0,"used":"corrupt"});
        pending.rows.push(put("requests", "0".into(), binding));
        assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        pending.rows.push(RowChange::Delete { table: "requests".into(), id: "0".into() });
        assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        let mut binding = request_row(&snapshot, &Commit::new("read", json!({}), &snapshot), NodeId(0)).unwrap();
        binding["extra_metadata"] = json!("pending wins");
        pending.rows.push(put("requests", "0".into(), binding));
        assert!(load_budget(&snapshot, &pending, NodeId(0)).unwrap().used());
        save_budget(&snapshot, &mut pending, NodeId(0), ReplanBudget::import(NodeId(0), true)).unwrap();
        assert_eq!(request_row(&snapshot, &pending, NodeId(0)).unwrap()["extra_metadata"], "pending wins");
    }
}
