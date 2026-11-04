//! Counts-only store diagnostics for #783.
use serde::Serialize;
#[derive(Default, Debug, Serialize, PartialEq, Eq)]
pub struct Stats {
    pub world_rows: std::collections::BTreeMap<String, i64>,
    pub events: i64,
    pub min_event_revision: Option<i64>,
    pub max_event_revision: Option<i64>,
    pub min_world_version: Option<i64>,
    pub replay_from_revision: Option<i64>,
}
/// #783: same SQL-enforced READ ONLY, REPEATABLE READ transaction as replay.
/// Only counts, names of tables, and revision numbers leave this function.
pub fn load(reader: &mut tl3_store::Reader) -> crate::Result<Stats> {
    let mut tx = reader.transaction()?;
    let stats = read(&mut tx)?;
    tx.commit().map_err(tl3_store::Error::from)?;
    Ok(stats)
}

pub fn read(tx: &mut tl3_store::Transaction<'_>) -> crate::Result<Stats> {
    let world_rows = tx
        .query(
            "SELECT table_name, count(*) FROM tl3_world GROUP BY table_name ORDER BY table_name",
            &[],
        )
        .map_err(tl3_store::Error::from)?
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    let events = tx
        .query_one(
            "SELECT count(*), min(revision), max(revision) FROM tl3_events",
            &[],
        )
        .map_err(tl3_store::Error::from)?;
    let min_world_version = tx
        .query_one("SELECT min(version) FROM tl3_world", &[])
        .map_err(tl3_store::Error::from)?
        .get(0);
    // Bounded pages avoid loading old payloads or printing any JSON value.
    // Use exactly replay's decoder, rather than an approximation of its schema.
    let mut cursor = 0i64;
    let replay_from_revision = 'search: loop {
        let records=tx.query("SELECT revision, result FROM tl3_events WHERE revision > $1 AND (result ? 'kernel_anchor' OR result ? 'kernel_replay') ORDER BY revision LIMIT 32",&[&cursor]).map_err(tl3_store::Error::from)?;
        if records.is_empty() {
            break None;
        }
        for record in records {
            cursor = record.get(0);
            let value: serde_json::Value = record.get(1);
            if matches!(crate::replay::stored_snapshot(&value), Ok(Some(_))) {
                break 'search Some(cursor);
            }
        }
    };
    Ok(Stats {
        world_rows,
        events: events.get(0),
        min_event_revision: events.get(1),
        max_event_revision: events.get(2),
        min_world_version,
        replay_from_revision,
    })
}

pub fn render_text(stats: &Stats) -> String {
    use std::fmt::Write;
    let mut output = String::from("metric\ttable\tcount\tmin_revision\tmax_revision\n");
    for (table, count) in &stats.world_rows {
        let _ = writeln!(
            output,
            "world_rows\t{}\t{count}\t-\t-",
            crate::board::plain(table)
        );
    }
    let n = |value: Option<i64>| value.map_or_else(|| "-".into(), |n| n.to_string());
    let _ = writeln!(
        output,
        "events\t-\t{}\t{}\t{}",
        stats.events,
        n(stats.min_event_revision),
        n(stats.max_event_revision)
    );
    let _ = writeln!(
        output,
        "world_version\t-\t-\t{}\t-",
        n(stats.min_world_version)
    );
    let _ = writeln!(
        output,
        "replay_from\t-\t-\t{}\t-",
        n(stats.replay_from_revision)
    );
    output
}

pub fn command() -> crate::Result<()> {
    const USAGE: &str = "usage: tl3 store-stats --config FILE [--json]";
    let mut args = std::env::args().skip(2);
    let mut path = None;
    let mut json = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" if path.is_none() => {
                path = Some(args.next().ok_or(crate::Error::Invalid(USAGE))?)
            }
            "--json" if !json => json = true,
            _ => return Err(crate::Error::Invalid(USAGE)),
        }
    }
    let config = tl3_config::Config::load_status(path.ok_or(crate::Error::Invalid(USAGE))?)?;
    let stats = load(&mut tl3_store::Reader::open(&config.store)?)?;
    if json {
        println!("{}", serde_json::to_string(&stats)?);
    } else {
        print!("{}", render_text(&stats));
    }
    Ok(())
}
