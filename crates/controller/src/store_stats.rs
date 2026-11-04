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

pub fn read(_tx: &mut tl3_store::Transaction<'_>) -> crate::Result<Stats> {
    Ok(Stats::default())
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
