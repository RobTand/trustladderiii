//! Read-only replay verification for #783.
use serde::{Deserialize, Serialize};
use tl3_kernel::{Event, NodeId, PlannerResult, ReplanBudget, World};
use tl3_store::EventRecord;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRecord {
    pub input: World,
    pub event: Event,
    pub after: World,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub before: World,
    pub steps: Vec<StepRecord>,
    pub after: World,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannerStepRecord {
    pub input: World,
    pub node: NodeId,
    pub generation: u64,
    pub result: PlannerResult,
    pub budget: ReplanBudget,
    pub after_budget: ReplanBudget,
    pub after: World,
}

impl PlannerStepRecord {
    pub fn check(&self) -> Result<(), String> {
        if !self.budget.bound_to(self.node) || !self.after_budget.bound_to(self.node) {
            return Err("planner budget binding diverged".into());
        }
        check_world(&self.input)?;
        let (world, budget, _) = tl3_kernel::step_planner(
            self.input.clone(), self.budget, self.node, self.generation, self.result,
        );
        check_world(&world)?;
        if world != self.after || budget != self.after_budget {
            return Err("planner step snapshot or budget diverged".into());
        }
        Ok(())
    }
}

/// #783: the existing full kernel invariants, used only while reading replay.
pub fn check_world(world: &World) -> Result<(), String> {
    world.check().map_err(|broken| format!("{broken:?}"))?;
    if !world.is_settled() {
        return Err("Unsettled".into());
    }
    Ok(())
}

/// The stored starting snapshot, shared by replay and counts-only diagnostics.
/// Availability means a decodable anchor or complete ReplayRecord; it does not
/// certify the suffix. Replay still checks its invariants and every real step.
pub(crate) enum StoredSnapshot {
    Anchor(World),
    Replay(ReplayRecord),
    Planner(PlannerStepRecord),
}
pub(crate) fn stored_snapshot(
    result: &serde_json::Value,
) -> Result<Option<StoredSnapshot>, &'static str> {
    if let Some(anchor) = result.get("kernel_anchor") {
        return serde_json::from_value(anchor.clone())
            .map(StoredSnapshot::Anchor)
            .map(Some)
            .map_err(|_| "invalid anchor");
    }
    if let Some(value) = result.get("planner_replay") {
        return serde_json::from_value(value.clone())
            .map(StoredSnapshot::Planner).map(Some).map_err(|_| "invalid planner replay");
    }
    result
        .get("kernel_replay")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map(StoredSnapshot::Replay)
                .map_err(|_| "invalid replay record")
        })
        .transpose()
}

/// #783: replay real kernel steps, compare each snapshot, and retain chain continuity.
pub fn check_record(
    record: &EventRecord,
    previous: Option<&World>,
) -> Result<Option<World>, String> {
    let result = &record.receipt.result;
    let fail = |reason: &str| {
        format!(
            "event {} (revision {}): {reason}",
            record.key,
            record.receipt.revision.get()
        )
    };
    if result.get("kernel_violation").is_some() {
        return Err(fail("recorded kernel invariant violation"));
    }
    if let Some(StoredSnapshot::Planner(planner)) = stored_snapshot(result).map_err(fail)? {
        planner.check().map_err(|error| fail(&error))?;
        if let Some(world) = previous {
            check_prepared(world, &planner.input).map_err(|reason| fail(&reason))?;
        }
        if let Some(metadata) = result.get("kernel_planner_input") {
            if *metadata != serde_json::json!({"node":planner.node,"generation":planner.generation,"result":planner.result}) {
                return Err(fail("planner input metadata diverged"));
            }
        }
        return Ok(Some(planner.after));
    }
    let replay = match stored_snapshot(result).map_err(fail)? {
        Some(StoredSnapshot::Anchor(world)) => {
            if previous.is_some() {
                return Err(fail("unexpected replay anchor"));
            }
            check_world(&world).map_err(|rule| fail(&format!("invariant {rule}")))?;
            return Ok(Some(world));
        }
        Some(StoredSnapshot::Replay(replay)) => replay,
        Some(StoredSnapshot::Planner(_)) => unreachable!("planner replay checked above"),
        None => {
            if result.get("kernel_event").is_some()
                || result.get("kernel_planner_input").is_some()
                || result.get("dependency_events").is_some()
                || record.payload.get("initialize").is_some()
                || record.payload.get("gate_capacity").is_some()
            {
                return Err(fail(
                    "legacy history without recorded snapshots; historical snapshot unavailable; choose --from at an instrumented event",
                ));
            }
            return Ok(None);
        }
    };
    check_world(&replay.before).map_err(|rule| fail(&format!("baseline invariant {rule}")))?;
    if previous.is_some_and(|previous| previous != &replay.before) {
        return Err(fail("snapshot chain diverged"));
    }
    let mut world = replay.before;
    if let Some(input) = result.get("kernel_settle") {
        if !replay.steps.is_empty() {
            return Err(fail("settle record also contains events"));
        }
        let input: World =
            serde_json::from_value(input.clone()).map_err(|_| fail("invalid capacity snapshot"))?;
        // Capacity reconciliation samples config, never changes request state.
        if input.nodes != world.nodes
            || input.file_sets() != world.file_sets()
            || input.dependencies() != world.dependencies()
            || input.next_ticket != world.next_ticket
            || input.next_question != world.next_question
        {
            return Err(fail("capacity preparation changed request snapshot"));
        }
        world = tl3_kernel::settle_world(input).0;
        check_world(&world).map_err(|rule| fail(&format!("invariant {rule}")))?;
    }
    for step in replay.steps {
        let (next, _) = tl3_kernel::step(step.input.clone(), step.event);
        check_world(&next).map_err(|rule| fail(&format!("invariant {rule}")))?;
        if next != step.after {
            return Err(fail("step snapshot diverged"));
        }
        check_prepared(&world, &step.input).map_err(|reason| fail(&reason))?;
        world = next;
    }
    if world != replay.after {
        return Err(fail("stored snapshot diverged"));
    }
    Ok(Some(world))
}

/// Translation may append interned footprints before either public entry.
fn check_prepared(world: &World, input: &World) -> Result<(), String> {
    let mut prepared = serde_json::to_value(world).map_err(|_| "cannot encode baseline")?;
    prepared["file_sets"] = serde_json::to_value(input.file_sets()).map_err(|_| "cannot encode footprints")?;
    if !input.file_sets().starts_with(world.file_sets())
        || prepared != serde_json::to_value(input).map_err(|_| "cannot encode input")? {
        return Err("step input snapshot diverged".into());
    }
    Ok(())
}

fn check_budget_chain(record: &EventRecord, budgets: &mut std::collections::BTreeMap<u64, ReplanBudget>) -> Result<(), String> {
    if let Some(value) = record.receipt.result.get("planner_replay") {
        let planner: PlannerStepRecord = serde_json::from_value(value.clone()).map_err(|_| "invalid planner replay")?;
        if budgets.get(&planner.node.0).is_some_and(|budget| *budget != planner.budget) {
            return Err(format!("event {}: planner budget chain diverged", record.key));
        }
        budgets.insert(planner.node.0, planner.after_budget);
    }
    Ok(())
}

#[derive(Debug)]
pub struct Report {
    pub events: u64,
    pub steps: u64,
    pub revision: i64,
}

/// #783: one repeatable-read READ ONLY transaction covers the log and final rows.
/// No Store writer, migration, controller ownership, or upstream adapter is opened.
pub fn run(reader: &mut tl3_store::Reader, from: Option<&str>) -> crate::Result<Report> {
    use crate::Error;
    use tl3_store::Revision;
    let mut tx = reader.transaction()?;
    let snapshot = tl3_store::load_snapshot(&mut tx)?;
    let mut cursor = match from {
        None => Revision::ZERO,
        Some(event) => {
            let revision = tl3_store::find_event_revision(&mut tx, event)?
                .ok_or_else(|| Error::Replay(format!("event {event}: not found")))?;
            Revision::try_from(revision.get() - 1)?
        }
    };
    let mut previous = None;
    let mut budgets = std::collections::BTreeMap::new();
    let mut report = Report {
        events: 0,
        steps: 0,
        revision: snapshot.revision.get(),
    };
    let mut last_key = String::from("no event");
    loop {
        let records = tl3_store::events_in(&mut tx, cursor, 128)?;
        if records.is_empty() {
            break;
        }
        for record in records {
            cursor = record.receipt.revision;
            last_key = record.key.clone();
            if from.is_none()
                && previous.is_none()
                && (record.receipt.result.get("kernel_replay").is_some()
                    || record.receipt.result.get("planner_replay").is_some())
            {
                return Err(Error::Replay(format!(
                    "event {last_key}: missing initial anchor; choose --from explicitly"
                )));
            }
            check_budget_chain(&record, &mut budgets).map_err(Error::Replay)?;
            if let Some(world) = check_record(&record, previous.as_ref()).map_err(Error::Replay)? {
                report.steps += record.receipt.result["kernel_replay"]["steps"]
                    .as_array()
                    .map_or(0, |steps| steps.len() as u64)
                    + u64::from(record.receipt.result.get("planner_replay").is_some());
                previous = Some(world);
            }
            report.events += 1;
        }
    }
    let expected = previous
        .ok_or_else(|| Error::Replay("no replayable kernel snapshot in selected history".into()))?;
    let expected_rows = crate::snapshot::encode(&expected)?;
    let mismatch = snapshot
        .rows
        .iter()
        .filter(|row| {
            matches!(
                row.table.as_str(),
                "kernel" | "nodes" | "gates" | "file_sets"
            )
        })
        .filter(|row| expected_rows.get(&(row.table.clone(), row.id.clone())) != Some(&row.value))
        .map(|row| row.version)
        .min();
    if let Some(revision) = mismatch {
        if let Some(record) = tl3_store::events_in(
            &mut tx,
            Revision::try_from(revision.get().saturating_sub(1))?,
            1,
        )?
        .first()
        {
            last_key = record.key.clone();
        }
        return Err(Error::Replay(format!(
            "event {last_key}: final stored snapshot diverged at revision {}",
            revision.get()
        )));
    }
    let actual = crate::snapshot::decode(&snapshot)
        .map_err(|error| Error::Replay(format!("event {last_key}: final snapshot: {error}")))?;
    if expected != actual {
        return Err(Error::Replay(format!(
            "event {last_key}: final stored snapshot diverged at revision {}",
            snapshot.revision.get()
        )));
    }
    let read = tl3_store::Commit::new("replay-read", serde_json::json!({}), &snapshot);
    for index in 0..expected.nodes.len() {
        let node = NodeId(index as u64);
        let budget = crate::planner_input::load_budget(&snapshot, &read, node)
            .map_err(|error| Error::Replay(format!("event {last_key}: final budget {}: {error}", node.0)))?;
        if budgets.get(&node.0).is_some_and(|expected| *expected != budget) {
            return Err(Error::Replay(format!("event {last_key}: final planner budget {} diverged", node.0)));
        }
    }
    tx.commit().map_err(tl3_store::Error::from)?;
    Ok(report)
}

pub fn command() -> crate::Result<()> {
    use crate::Error;
    const USAGE: &str = "usage: tl3 replay-check --config FILE [--from EVENT]";
    let mut args = std::env::args().skip(2);
    let mut config = None;
    let mut from = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" if config.is_none() => {
                config = Some(args.next().ok_or(Error::Invalid(USAGE))?)
            }
            "--from" if from.is_none() => from = Some(args.next().ok_or(Error::Invalid(USAGE))?),
            _ => return Err(Error::Invalid(USAGE)),
        }
    }
    let config = tl3_config::Config::load_status(config.ok_or(Error::Invalid(USAGE))?)?;
    let report = run(
        &mut tl3_store::Reader::open(&config.store)?,
        from.as_deref(),
    )?;
    println!(
        "replay-check OK: {} events, {} kernel steps, through revision {}",
        report.events, report.steps, report.revision
    );
    Ok(())
}
