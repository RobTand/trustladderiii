//! Read-only replay verification for #783.
use serde::{Deserialize, Serialize};
use tl3_kernel::{Event, World};
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
    let replay = match stored_snapshot(result).map_err(fail)? {
        Some(StoredSnapshot::Anchor(world)) => {
            if previous.is_some() {
                return Err(fail("unexpected replay anchor"));
            }
            check_world(&world).map_err(|rule| fail(&format!("invariant {rule}")))?;
            return Ok(Some(world));
        }
        Some(StoredSnapshot::Replay(replay)) => replay,
        None => {
            if result.get("kernel_event").is_some()
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
        // Translation may intern footprints before a step. All other fields
        // must come from the preceding real kernel result.
        let mut prepared =
            serde_json::to_value(&world).map_err(|_| fail("cannot encode baseline"))?;
        prepared["file_sets"] = serde_json::to_value(step.input.file_sets())
            .map_err(|_| fail("cannot encode footprints"))?;
        if !step.input.file_sets().starts_with(world.file_sets())
            || prepared
                != serde_json::to_value(&step.input).map_err(|_| fail("cannot encode input"))?
        {
            return Err(fail("step input snapshot diverged"));
        }
        world = next;
    }
    if world != replay.after {
        return Err(fail("stored snapshot diverged"));
    }
    Ok(Some(world))
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
                && record.receipt.result.get("kernel_replay").is_some()
            {
                return Err(Error::Replay(format!(
                    "event {last_key}: missing initial anchor; choose --from explicitly"
                )));
            }
            if let Some(world) = check_record(&record, previous.as_ref()).map_err(Error::Replay)? {
                report.steps += record.receipt.result["kernel_replay"]["steps"]
                    .as_array()
                    .map_or(0, |steps| steps.len() as u64);
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
