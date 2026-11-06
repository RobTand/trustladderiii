//! Counts-only store diagnostics for #783, with an explicit scope-trim export for #896.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
    const USAGE: &str = "usage: tl3 store-stats --config FILE [--json] [--scope-trims]\n--scope-trims emits one JSON document with rows and summary; written_at contains revision and UTC committed_at.";
    let mut args = std::env::args().skip(2);
    let mut path = None;
    let mut json = false;
    let mut scope_trims = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" if path.is_none() => {
                path = Some(args.next().ok_or(crate::Error::Invalid(USAGE))?)
            }
            "--json" if !json => json = true,
            "--scope-trims" if !scope_trims => scope_trims = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(crate::Error::Invalid(USAGE)),
        }
    }
    let config = tl3_config::Config::load_status(path.ok_or(crate::Error::Invalid(USAGE))?)?;
    let mut reader = tl3_store::Reader::open(&config.store)?;
    if scope_trims {
        println!(
            "{}",
            serde_json::to_string(&load_scope_trims(&mut reader)?)?
        );
        return Ok(());
    }
    let stats = load(&mut reader)?;
    if json {
        println!("{}", serde_json::to_string(&stats)?);
    } else {
        print!("{}", render_text(&stats));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrimValue {
    head: String,
    removed_paths: Vec<String>,
}

#[derive(Default, Serialize)]
struct Counts {
    total_rows: usize,
    non_empty_rows: usize,
}
impl Counts {
    fn add(&mut self, non_empty: bool) {
        self.total_rows += 1;
        self.non_empty_rows += usize::from(non_empty);
    }
}

/// #896 F0: one JSON document with `rows` and `summary`, ordered by node and group.
/// Only scope_trims values, request issue numbers and repository names leave here.
/// `written_at` contains the latest write's revision and UTC commit timestamp.
/// A missing event preserves the trim and emits a null commit timestamp.
/// Missing or malformed owning fields are null. Invalid trims count toward totals
/// in their owning groups, but never toward non-empty counts.
pub fn load_scope_trims(reader: &mut tl3_store::Reader) -> crate::Result<Value> {
    let mut tx = reader.transaction()?;
    let export = read_scope_trims(&mut tx)?;
    tx.commit().map_err(tl3_store::Error::from)?;
    Ok(export)
}

pub fn read_scope_trims(tx: &mut tl3_store::Transaction<'_>) -> crate::Result<Value> {
    // Request bindings use the delivery node id, including bindings copied to
    // split children. Project just their issue and repo names, never the title,
    // installation, source, worker inputs or other world/event values.
    let records = tx
        .query(
            r#"SELECT trim.row_id, trim.value, trim.version,
                  to_char(event.committed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
                  request.value -> 'issue', request.value #> '{repo,owner}',
                  request.value #> '{repo,name}'
           FROM tl3_world trim
           LEFT JOIN tl3_events event ON event.revision = trim.version
           LEFT JOIN tl3_world request ON request.table_name = 'requests'
                                      AND request.row_id = trim.row_id
           WHERE trim.table_name = 'scope_trims'
           ORDER BY trim.row_id"#,
            &[],
        )
        .map_err(tl3_store::Error::from)?;
    let mut rows = Vec::with_capacity(records.len());
    let mut total = Counts::default();
    let mut by_repo = BTreeMap::<Option<String>, Counts>::new();
    let mut by_request = BTreeMap::<(Option<String>, Option<u64>), Counts>::new();
    for record in records {
        let node: String = record.get(0);
        let value: Value = record.get(1);
        let trim = serde_json::from_value::<TrimValue>(value).ok();
        let request = record
            .get::<_, Option<Value>>(4)
            .and_then(|value| value.as_u64());
        let name = |column| {
            record
                .get::<_, Option<Value>>(column)
                .and_then(|value| value.as_str().map(str::to_owned))
        };
        let repo = name(5)
            .zip(name(6))
            .map(|(owner, name)| format!("{owner}/{name}"));
        let non_empty = trim
            .as_ref()
            .is_some_and(|trim| !trim.removed_paths.is_empty());
        total.add(non_empty);
        by_repo.entry(repo.clone()).or_default().add(non_empty);
        by_request
            .entry((repo.clone(), request))
            .or_default()
            .add(non_empty);
        rows.push(match trim {
            Some(trim) => json!({
                "node":node, "request":request, "repo":repo,
                "head":trim.head, "removed_paths":trim.removed_paths,
                "written_at":{"revision":record.get::<_, i64>(2), "committed_at":record.get::<_, Option<String>>(3)}
            }),
            None => json!({"node":node,"invalid":true}),
        });
    }
    let by_repo: Vec<_> = by_repo
        .into_iter()
        .map(|(repo, count)| {
            json!({
                "repo":repo,"total_rows":count.total_rows,"non_empty_rows":count.non_empty_rows
            })
        })
        .collect();
    let by_request: Vec<_> = by_request.into_iter().map(|((repo, request), count)| json!({
        "repo":repo,"request":request,"total_rows":count.total_rows,"non_empty_rows":count.non_empty_rows
    })).collect();
    Ok(json!({"rows":rows,"summary":{
        "total_rows":total.total_rows,"non_empty_rows":total.non_empty_rows,
        "by_repo":by_repo,"by_request":by_request
    }}))
}
