use crate::{Error, Result, decode_stored, put};
use serde_json::Value;
use tl3_kernel::{Event, NodeId, PlannerResult, ReplanBudget};
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Copy, Debug)]
pub(crate) enum KernelInput {
    Event(Event),
    Planner {
        node: NodeId,
        generation: u64,
        result: PlannerResult,
    },
}

impl From<Event> for KernelInput {
    fn from(event: Event) -> Self {
        Self::Event(event)
    }
}

#[cfg(test)]
std::thread_local! {
    static REQUEST_ROW_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Pending deletion is authoritative, just like a pending replacement row.
pub(crate) fn request_row(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Value> {
    #[cfg(test)]
    REQUEST_ROW_LOADS.set(REQUEST_ROW_LOADS.get() + 1);
    let id = node.0.to_string();
    for change in commit.rows.iter().rev() {
        match change {
            RowChange::Put {
                table,
                id: key,
                value,
            } if table == "requests" && *key == id => {
                return Ok(value.clone());
            }
            RowChange::Delete { table, id: key } if table == "requests" && *key == id => {
                return Err(Error::Invalid("deleted request binding"));
            }
            _ => {}
        }
    }
    snapshot
        .rows
        .iter()
        .find(|row| row.table == "requests" && row.id == id)
        .map(|row| row.value.clone())
        .ok_or(Error::Invalid("missing request binding"))
}

pub(crate) fn load_budget(
    snapshot: &Snapshot,
    commit: &Commit,
    node: NodeId,
) -> Result<ReplanBudget> {
    let row = request_row(snapshot, commit, node)?;
    if row
        .get("replan_budget_format")
        .is_some_and(|format| format != 1)
    {
        return Err(Error::Invalid("unsupported replan budget format"));
    }
    let legacy = snapshot
        .rows
        .iter()
        .find(|row| row.table == "nodes" && row.id == node.0.to_string())
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
            if row.get("replan_budget_format").is_some()
                || snapshot.rows.iter().any(|old| {
                    old.table == "requests"
                        && old.id == node.0.to_string()
                        && (old.value.get("replan_budget").is_some()
                            || old.value.get("replan_budget_format").is_some())
                })
            {
                return Err(Error::Invalid("removed request replan budget"));
            }
            // Inspect raw legacy storage before World decoding drops the old field.
            ReplanBudget::import(node, consumed)
        }
    };
    if !budget.bound_to(node) {
        return Err(Error::Invalid("replan budget binding mismatch"));
    }
    if let Some(old) = snapshot
        .rows
        .iter()
        .find(|row| row.table == "requests" && row.id == node.0.to_string())
        && let Some(value) = old.value.get("replan_budget")
    {
        let persisted = decode_stored::<ReplanBudget>(value.clone())?;
        if !persisted.bound_to(node) || (persisted.used() && !budget.used() && !consumed) {
            return Err(Error::Invalid("replan budget reset"));
        }
    }
    Ok(if consumed {
        ReplanBudget::import(node, true)
    } else {
        budget
    })
}

pub(crate) fn save_budget(
    snapshot: &Snapshot,
    commit: &mut Commit,
    node: NodeId,
    budget: ReplanBudget,
) -> Result<()> {
    if !budget.bound_to(node) {
        return Err(Error::Invalid("replan budget binding mismatch"));
    }
    if let Some(old) = snapshot
        .rows
        .iter()
        .find(|row| row.table == "requests" && row.id == node.0.to_string())
        && let Some(value) = old.value.get("replan_budget")
    {
        let persisted = decode_stored::<ReplanBudget>(value.clone())?;
        if !persisted.bound_to(node) || (persisted.used() && !budget.used()) {
            return Err(Error::Invalid("replan budget reset"));
        }
    }
    let mut row = request_row(snapshot, commit, node)?;
    let fields = row
        .as_object_mut()
        .ok_or(Error::Invalid("invalid request binding"))?;
    let encoded = serde_json::to_value(budget)?;
    if fields.get("replan_budget") == Some(&encoded)
        && fields.get("replan_budget_format") == Some(&serde_json::json!(1))
    {
        return Ok(());
    }
    fields.insert("replan_budget".into(), encoded);
    fields.insert("replan_budget_format".into(), serde_json::json!(1));
    commit.rows.push(put("requests", node.0.to_string(), row));
    Ok(())
}

/// Preserve removed legacy state only when this commit rewrites its node row.
pub(crate) fn import_rewritten_legacy(
    snapshot: &Snapshot,
    commit: &mut Commit,
    changes: &[RowChange],
) -> Result<()> {
    let legacy: std::collections::BTreeMap<_, _> = snapshot.rows.iter()
        .filter(|row| row.table == "nodes")
        .filter_map(|row| row.value.get("empty_allowed_replanned").map(|flag| (&row.id, flag)))
        .collect();
    for change in changes {
        let RowChange::Put { table, id, .. } = change else { continue };
        if table != "nodes" { continue; }
        match legacy.get(id) {
            Some(Value::Bool(true)) => {
                let node = NodeId(id.parse().map_err(|_| Error::Invalid("invalid legacy node id"))?);
                let budget = load_budget(snapshot, commit, node)?;
                save_budget(snapshot, commit, node, budget)?;
            }
            Some(Value::Bool(false)) | None => {}
            Some(_) => return Err(Error::Invalid("invalid legacy replan budget")),
        }
    }
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
        Bootstrap {
            operations: None,
            repos: vec![Repository {
                owner: "owner".into(),
                name: "repo".into(),
                installation: 1,
            }],
            rob: 1,
            slots: 1,
            pool: 1,
            fail_fast,
        }
    }

    fn confirm(world: &mut World) {
        for _ in 0..32 {
            let Some(effect) = world.nodes[0].effect else {
                return;
            };
            *world = step(
                world.clone(),
                Event::Ack {
                    node: NodeId(0),
                    seq: effect.seq,
                    ack: Ack::Confirmed,
                },
            )
            .0;
        }
        panic!("fixture did not settle effects");
    }

    fn planning(fail_fast: bool) -> World {
        let mut w = step(
            World::new(&[1], 1),
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::Unranked,
            },
        )
        .0;
        w.fail_fast = fail_fast;
        confirm(&mut w);
        let generation = w.nodes[0].generation;
        w = step(
            w,
            Event::Reply {
                node: NodeId(0),
                generation,
                reply: Reply::IngestDone { size: 2 },
            },
        )
        .0;
        confirm(&mut w);
        let generation = w.nodes[0].generation;
        w = step(
            w,
            Event::Reply {
                node: NodeId(0),
                generation,
                reply: Reply::AuditAgrees,
            },
        )
        .0;
        confirm(&mut w);
        assert!(matches!(w.nodes[0].phase, Phase::Planning { .. }));
        w.check().unwrap();
        assert!(w.is_settled());
        w
    }

    fn seed(c: &mut Controller, world: World, legacy: bool) {
        c.intake(LabelledIssue {
            delivery: "seed".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            installation: 1,
            number: 930,
            title: "Budget".into(),
            actor: 1,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
        let snapshot = c.store.load().unwrap();
        let mut commit = Commit::new("seed-planning", json!({}), &snapshot);
        commit.rows.extend(snapshot::changes(
            &snapshot::encode(&snapshot::decode(&snapshot).unwrap()).unwrap(),
            snapshot::encode(&world).unwrap(),
        ));
        let mut binding = request_row(&snapshot, &commit, NodeId(0)).unwrap();
        binding.as_object_mut().unwrap().remove("replan_budget");
        binding
            .as_object_mut()
            .unwrap()
            .remove("replan_budget_format");
        binding["extra_metadata"] = json!({"retain": [1, 2, 3]});
        commit.rows.push(put("requests", "0".into(), binding));
        if legacy {
            let mut raw = serde_json::to_value(world.nodes[0]).unwrap();
            raw["empty_allowed_replanned"] = json!(true);
            commit.rows.push(put("nodes", "0".into(), raw));
        }
        c.store.commit(&commit).unwrap();
    }

    fn prepare(
        c: &mut Controller,
        snapshot: &Snapshot,
        key: &str,
        generation: u64,
        result: PlannerResult,
    ) -> Commit {
        let mut commit = Commit::new(key, json!({"planner":key}), snapshot);
        c.apply_input(
            snapshot,
            snapshot::decode(snapshot).unwrap(),
            KernelInput::Planner {
                node: NodeId(0),
                generation,
                result,
            },
            &mut commit,
        )
        .unwrap();
        commit
    }

    #[test]
    fn planner_budget_real_store_conflict_reopen_duplicate_and_new_version() {
        let mut database = support::Database::start();
        for fail_fast in [false, true] {
            let settings = database.create(&format!("rp_budget_{fail_fast}"));
            let root = tempfile::tempdir().unwrap();
            let mut c = Controller::open(
                Store::open(&settings).unwrap(),
                adapters::simulation(root.path()).unwrap(),
                bootstrap(fail_fast),
            )
            .unwrap();
            seed(&mut c, planning(fail_fast), false);
            let snapshot = c.store.load().unwrap();
            let generation = snapshot::decode(&snapshot).unwrap().nodes[0].generation;
            let cases: Vec<Value> =
                serde_json::from_str(include_str!("../tests/fixtures/pq-2648-empty-replan.json"))
                    .unwrap();
            let mut queue = Commit::new("race-input", json!({}), &snapshot);
            queue.rows.push(put("inbox", "race-delivery".into(), json!({"WorkerReply": {
                "node":0,"generation":generation,"role":"planner","reply":cases[0]["reply"],"model":null,
                "plan_check":{"objections":[{"allowed_empty":true,"class":"other","rule":"proof.omitted","items":["proof"],"diagnostic":"missing proof"}],"repair":{"status":"attempted"}}
            }})));
            c.store.commit(&queue).unwrap();
            let start = c.store.load().unwrap();
            let before = snapshot::decode(&start).unwrap();
            let generation = before.nodes[0].generation;
            let first = prepare(
                &mut c,
                &start,
                "first",
                generation,
                PlannerResult::EmptyAllowedReplan,
            );
            let losing = prepare(
                &mut c,
                &start,
                "losing",
                generation,
                PlannerResult::EmptyAllowedReplan,
            );
            c.store.commit(&first).unwrap();
            assert!(matches!(
                c.store.commit(&losing),
                Err(tl3_store::Error::Stale { .. })
            ));
            let row = start
                .rows
                .iter()
                .find(|r| r.table == "inbox" && r.id == "race-delivery")
                .unwrap();
            assert!(!c.consume_kernel_input(&start, row).unwrap());
            let fresh = c.store.load().unwrap();
            let row = fresh
                .rows
                .iter()
                .find(|r| r.table == "inbox" && r.id == "race-delivery")
                .unwrap();
            assert!(c.consume_kernel_input(&fresh, row).unwrap());
            let persisted = c.store.load().unwrap();
            let budget = load_budget(
                &persisted,
                &Commit::new("read", json!({}), &persisted),
                NodeId(0),
            )
            .unwrap();
            assert!(budget.used());
            let request = persisted
                .rows
                .iter()
                .find(|r| r.table == "requests" && r.id == "0")
                .unwrap();
            let node = persisted
                .rows
                .iter()
                .find(|r| r.table == "nodes" && r.id == "0")
                .unwrap();
            assert_eq!(request.version, node.version);
            assert_eq!(
                request.value["extra_metadata"],
                json!({"retain": [1, 2, 3]})
            );
            let journal: crate::replay::PlannerStepRecord =
                serde_json::from_value(first.result["planner_replay"].clone()).unwrap();
            journal.check().unwrap();
            assert_eq!(journal.after_budget, budget);
            let duplicate = prepare(
                &mut c,
                &persisted,
                "duplicate",
                generation,
                PlannerResult::EmptyAllowedReplan,
            );
            c.store.commit(&duplicate).unwrap();
            assert_eq!(
                snapshot::decode(&c.store.load().unwrap()).unwrap(),
                journal.after
            );
            drop(c);
            let mut c = Controller::open(
                Store::open(&settings).unwrap(),
                adapters::simulation(root.path()).unwrap(),
                bootstrap(fail_fast),
            )
            .unwrap();
            let snapshot = c.store.load().unwrap();
            let mut after = snapshot::decode(&snapshot).unwrap();
            confirm(&mut after);
            let mut ack = Commit::new("confirm-replan", json!({}), &snapshot);
            ack.rows.extend(snapshot::changes(
                &snapshot::encode(&snapshot::decode(&snapshot).unwrap()).unwrap(),
                snapshot::encode(&after).unwrap(),
            ));
            c.store.commit(&ack).unwrap();
            let snapshot = c.store.load().unwrap();
            let repeated = prepare(
                &mut c,
                &snapshot,
                "repeat",
                after.nodes[0].generation,
                PlannerResult::EmptyAllowedReplan,
            );
            c.store.commit(&repeated).unwrap();
            let snapshot = c.store.load().unwrap();
            let stopped = snapshot::decode(&snapshot).unwrap();
            assert!(matches!(
                stopped.nodes[0].phase,
                Phase::Stopping(_) | Phase::Closed(_)
            ));
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
            assert!(
                crate::replay::run(&mut reader, Some("repeat"))
                    .unwrap_err()
                    .to_string()
                    .contains("budget")
            );
            let mut restore = Commit::new("restore-budget", json!({}), &c.store.load().unwrap());
            restore.rows.push(put(
                "requests",
                "0".into(),
                request_row(&snapshot, &restore, NodeId(0)).unwrap(),
            ));
            c.store.commit(&restore).unwrap();
            let snapshot = c.store.load().unwrap();
            let mut replacement = Commit::new("request-replacement", json!({}), &snapshot);
            let issue: tl3_github::Issue = serde_json::from_value(json!({"number":930,
                "title":"Budget replacement","body":"New request version","state":"open",
                "updated_at":"2026-11-07T00:00:00Z"}))
            .unwrap();
            replacement.rows.push(put(
                "replacement_pending",
                "0".into(),
                json!(crate::steering::Task::Restart {
                    comment_key: "replacement-comment".into(),
                    text: "restart".into(),
                    issue: Some(issue),
                }),
            ));
            replacement.rows.push(put(
                "request_decisions",
                "0".into(),
                serde_json::from_str(include_str!("../tests/fixtures/decisions/pq2662.json"))
                    .unwrap(),
            ));
            c.store.commit(&replacement).unwrap();
            let snapshot = c.store.load().unwrap();
            let mut version = Commit::new("new-version", json!({}), &snapshot);
            let labelled = LabelledIssue {
                delivery: "new-version".into(),
                owner: "owner".into(),
                repo: "repo".into(),
                installation: 1,
                number: 930,
                title: "Budget replacement".into(),
                actor: 1,
                label: "tl3".into(),
                labels: vec![],
            };
            let event = c
                .prepare_intake(
                    &snapshot,
                    &labelled,
                    bootstrap(fail_fast).repos.remove(0),
                    None,
                    &mut version,
                )
                .unwrap()
                .unwrap();
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
    fn planner_budget_controller_later_failures_fit_leaf_stale_and_stopping() {
        let mut database = support::Database::start();
        for fail_fast in [false, true] {
            let settings = database.create(&format!("rp_ordinary_{fail_fast}"));
            let root = tempfile::tempdir().unwrap();
            let mut c = Controller::open(
                Store::open(&settings).unwrap(),
                adapters::simulation(root.path()).unwrap(),
                bootstrap(fail_fast),
            )
            .unwrap();
            let mut world = planning(fail_fast);
            let repo = c.store.intern_repo("owner", "repo").unwrap();
            let file = c.store.intern_path(repo, "src/rp.rs").unwrap();
            let footprint = world.intern_footprint([FileId(file.get() as u64)]);
            seed(&mut c, world.clone(), true);
            let snapshot = c.store.load().unwrap();
            let generation = world.nodes[0].generation;
            for (reply, stops) in [
                (Reply::PlanCantMeet, true),
                (Reply::PlanSplit, true),
                (
                    Reply::PlanLeaf {
                        footprint,
                        fits: false,
                    },
                    true,
                ),
                (
                    Reply::PlanLeaf {
                        footprint: Footprint::default(),
                        fits: true,
                    },
                    true,
                ),
                (
                    Reply::PlanLeaf {
                        footprint,
                        fits: true,
                    },
                    false,
                ),
            ] {
                let commit = prepare(
                    &mut c,
                    &snapshot,
                    "ordinary",
                    generation,
                    PlannerResult::Ordinary(reply),
                );
                let journal: crate::replay::PlannerStepRecord =
                    serde_json::from_value(commit.result["planner_replay"].clone()).unwrap();
                journal.check().unwrap();
                assert!(journal.after_budget.used());
                if stops {
                    assert_eq!(
                        journal.after.nodes[0].review_stop,
                        Some(ReviewStop::PlanCantMeet)
                    );
                    assert_eq!(journal.after.nodes[0].credit, world.nodes[0].credit);
                    assert!(journal.after.nodes[0].question.is_none());
                } else {
                    assert_eq!(
                        journal.after,
                        step(
                            world.clone(),
                            Event::Reply {
                                node: NodeId(0),
                                generation,
                                reply
                            }
                        )
                        .0
                    );
                    assert!(journal.after.nodes[0].review_stop.is_none());
                }
            }
            let stale = prepare(
                &mut c,
                &snapshot,
                "stale-ordinary",
                generation - 1,
                PlannerResult::Ordinary(Reply::PlanCantMeet),
            );
            let stale: crate::replay::PlannerStepRecord =
                serde_json::from_value(stale.result["planner_replay"].clone()).unwrap();
            assert_eq!(
                stale.after,
                step(
                    world.clone(),
                    Event::Reply {
                        node: NodeId(0),
                        generation: generation - 1,
                        reply: Reply::PlanCantMeet
                    }
                )
                .0
            );
            let stopped = step(
                world,
                Event::Command {
                    node: NodeId(0),
                    id: 1,
                    command: Command::Stop,
                },
            )
            .0;
            let expected = step(
                stopped.clone(),
                Event::Reply {
                    node: NodeId(0),
                    generation,
                    reply: Reply::PlanCantMeet,
                },
            )
            .0;
            let mut completion = Commit::new("stopping-ordinary", json!({}), &snapshot);
            c.apply_input(
                &snapshot,
                stopped,
                KernelInput::Planner {
                    node: NodeId(0),
                    generation,
                    result: PlannerResult::Ordinary(Reply::PlanCantMeet),
                },
                &mut completion,
            )
            .unwrap();
            let journal: crate::replay::PlannerStepRecord =
                serde_json::from_value(completion.result["planner_replay"].clone()).unwrap();
            journal.check().unwrap();
            assert_eq!(journal.after, expected);
            assert!(journal.after.nodes[0].run.is_none() && !journal.after.nodes[0].pool);
        }
    }

    #[test]
    fn planner_budget_unrelated_corruption_does_not_block_input() {
        unrelated_rows(false);
    }

    #[test]
    fn planner_budget_request_row_loads_do_not_scale_with_world() {
        unrelated_rows(true);
    }

    fn unrelated_rows(count_only: bool) {
        let mut database = support::Database::start();
        for count in [2usize, 16] {
            let settings = database.create(&format!("rp_bounded_{count}_{count_only}"));
            let root = tempfile::tempdir().unwrap();
            let mut c = Controller::open(Store::open(&settings).unwrap(),
                adapters::simulation(root.path()).unwrap(), bootstrap(false)).unwrap();
            let mut world = planning(false);
            let n = world.nodes[0];
            world.nodes.resize(count, n);
            world.pool_capacity = count as u8;
            world.gates[0].w = count as u8;
            world.check().unwrap();
            seed(&mut c, world, false);
            let snapshot = c.store.load().unwrap();
            let mut setup = Commit::new("unrelated-rows", json!({}), &snapshot);
            let binding = request_row(&snapshot, &setup, NodeId(0)).unwrap();
            for index in 1..count {
                let mut row = binding.clone();
                if !count_only && index == count - 1 {
                    row["replan_budget"] = json!({"request":index,"used":"corrupt"});
                }
                setup.rows.push(put("requests", index.to_string(), row));
            }
            if count_only {
                let mut raw = serde_json::to_value(world_node(&snapshot, count - 1)).unwrap();
                raw["empty_allowed_replanned"] = json!(true);
                setup.rows.push(put("nodes", (count - 1).to_string(), raw));
            }
            c.store.commit(&setup).unwrap();
            let snapshot = c.store.load().unwrap();
            let before = snapshot::decode(&snapshot).unwrap();
            let mut commit = Commit::new("other-input", json!({}), &snapshot);
            REQUEST_ROW_LOADS.set(0);
            let result = c.apply(&snapshot, before, Event::Fact {
                node: NodeId(0), fact: Fact::Priority(Priority::P0),
            }, &mut commit);
            let loads = REQUEST_ROW_LOADS.get();
            result.expect("an unrelated request row must not block a kernel input");
            if count_only {
                assert_eq!(loads, 0, "non-planner input must not load unrelated request budgets");
            }
            assert!(!commit.rows.iter().any(|r| matches!(r, RowChange::Put { table, .. } if table == "requests")),
                "unrelated legacy request rows must remain untouched");
            c.store.commit(&commit).unwrap();
            assert_eq!(snapshot::decode(&c.store.load().unwrap()).unwrap().nodes[0].priority, Priority::P0);
            if count_only {
                let snapshot = c.store.load().unwrap();
                let mut rewrite = Commit::new("legacy-rewrite", json!({}), &snapshot);
                REQUEST_ROW_LOADS.set(0);
                c.apply(&snapshot, snapshot::decode(&snapshot).unwrap(), Event::Fact {
                    node: NodeId((count - 1) as u64), fact: Fact::Priority(Priority::P0),
                }, &mut rewrite).unwrap();
                assert_eq!(REQUEST_ROW_LOADS.get(), 2, "only the rewritten legacy row is loaded and saved");
                c.store.commit(&rewrite).unwrap();
                let snapshot = c.store.load().unwrap();
                let read = Commit::new("legacy-read", json!({}), &snapshot);
                assert!(load_budget(&snapshot, &read, NodeId((count - 1) as u64)).unwrap().used());
                assert!(snapshot.rows.iter().find(|r| r.table == "nodes" && r.id == (count - 1).to_string())
                    .unwrap().value.get("empty_allowed_replanned").is_none());
            }
        }
    }

    fn world_node(snapshot: &Snapshot, index: usize) -> Node {
        snapshot::decode(snapshot).unwrap().nodes[index]
    }

    #[test]
    fn planner_budget_legacy_import_pending_writes_and_corrupt_bindings() {
        let mut database = support::Database::start();
        let settings = database.create("rp_budget_legacy");
        let root = tempfile::tempdir().unwrap();
        let mut c = Controller::open(
            Store::open(&settings).unwrap(),
            adapters::simulation(root.path()).unwrap(),
            bootstrap(false),
        )
        .unwrap();
        seed(&mut c, planning(false), true);
        let snapshot = c.store.load().unwrap();
        let mut commit = Commit::new("legacy-generic-rewrite", json!({}), &snapshot);
        c.apply(
            &snapshot,
            snapshot::decode(&snapshot).unwrap(),
            Event::Fact {
                node: NodeId(0),
                fact: Fact::Priority(Priority::P0),
            },
            &mut commit,
        )
        .unwrap();
        c.store.commit(&commit).unwrap();
        let snapshot = c.store.load().unwrap();
        let mut pending = Commit::new("pending", json!({}), &snapshot);
        assert!(load_budget(&snapshot, &pending, NodeId(0)).unwrap().used());
        assert!(
            snapshot
                .rows
                .iter()
                .find(|r| r.table == "nodes" && r.id == "0")
                .unwrap()
                .value
                .get("empty_allowed_replanned")
                .is_none()
        );
        let mut binding = request_row(&snapshot, &pending, NodeId(0)).unwrap();
        for corrupt in [
            {
                let mut row = binding.clone();
                row.as_object_mut().unwrap().remove("replan_budget");
                row
            },
            {
                let mut row = binding.clone();
                row["replan_budget"] = json!(ReplanBudget::new(NodeId(0)));
                row
            },
            {
                let mut row = binding.clone();
                row["replan_budget_format"] = json!(99);
                row
            },
        ] {
            pending.rows.push(put("requests", "0".into(), corrupt));
            assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        }
        binding["replan_budget"] = json!(ReplanBudget::new(NodeId(1)));
        pending
            .rows
            .push(put("requests", "0".into(), binding.clone()));
        assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        binding["replan_budget"] = json!({"request":0,"used":"corrupt"});
        pending.rows.push(put("requests", "0".into(), binding));
        assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        pending.rows.push(RowChange::Delete {
            table: "requests".into(),
            id: "0".into(),
        });
        assert!(load_budget(&snapshot, &pending, NodeId(0)).is_err());
        let mut binding = request_row(
            &snapshot,
            &Commit::new("read", json!({}), &snapshot),
            NodeId(0),
        )
        .unwrap();
        binding["extra_metadata"] = json!("pending wins");
        pending.rows.push(put("requests", "0".into(), binding));
        assert!(load_budget(&snapshot, &pending, NodeId(0)).unwrap().used());
        save_budget(
            &snapshot,
            &mut pending,
            NodeId(0),
            ReplanBudget::import(NodeId(0), true),
        )
        .unwrap();
        assert_eq!(
            request_row(&snapshot, &pending, NodeId(0)).unwrap()["extra_metadata"],
            "pending wins"
        );
    }
}
