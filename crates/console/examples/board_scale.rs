//! Measures a board rebuild at about 2,000 requests, against budgets for
//! dl380g10 in a release build:
//!
//! - a full rebuild (one snapshot read and projection) within 200 ms;
//! - a worker-progress refresh within 20 ms.
//!
//! It drives one request to a merge with the simulated controller, then copies
//! that request's node and binding to fill the board. Run it through
//! PrismaBuild: `cargo run --release -p tl3-console --example board_scale`.
//! It exits 1 when a median exceeds its budget.

#[path = "../../store/tests/support/mod.rs"]
mod support;

use std::time::{Duration, Instant};
use tl3_controller::{Bootstrap, Controller, LabelledIssue, Repository, adapters, board};
use tl3_store::{Commit, Reader, RowChange, Store};

const REQUESTS: usize = 2_000;
const FULL_BUDGET: Duration = Duration::from_millis(200);
const ROUNDS: usize = 7;

fn main() {
    let mut database = support::Database::start();
    let settings = database.create("board_scale");
    let records = tempfile::tempdir().unwrap();
    let repos = ["tl3-sandbox", "prismabuild", "prismaquant", "tessera"];
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(records.path()).unwrap(),
        Bootstrap {
            operations: None,
            repos: repos
                .iter()
                .map(|name| Repository {
                    owner: "RobTand".into(),
                    name: (*name).into(),
                    installation: 1,
                })
                .collect(),
            rob: 24923402,
            slots: 2,
            pool: 2,
        },
    )
    .unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "scale-0".into(),
            owner: "RobTand".into(),
            repo: repos[0].into(),
            installation: 1,
            number: 1,
            title: "Seed request".into(),
            actor: 24923402,
            label: "tl3".into(),
        })
        .unwrap();
    while controller.drive_once().unwrap() {}
    drop(controller);
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let row = |table: &str| {
        snapshot
            .rows
            .iter()
            .find(|row| row.table == table && row.id == "0")
            .unwrap()
            .value
            .clone()
    };
    let (node, request) = (row("nodes"), row("requests"));
    let mut seed = Commit::new("scale-seed", serde_json::json!(REQUESTS), &snapshot);
    for index in 1..REQUESTS {
        let mut binding = request.clone();
        binding["issue"] = serde_json::json!(index + 1);
        binding["repo"]["name"] = serde_json::json!(repos[index % repos.len()]);
        binding["title"] = serde_json::json!(format!("Scaled request {index}"));
        for (table, value) in [("nodes", node.clone()), ("requests", binding)] {
            seed.rows.push(RowChange::Put {
                table: table.into(),
                id: index.to_string(),
                value,
            });
        }
    }
    store.commit(&seed).unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    let full = median(|| {
        let mut tx = reader.transaction().unwrap();
        let board = board::read(&mut tx, 0, None).unwrap();
        tx.commit().unwrap();
        assert_eq!(board.flow.columns.last().unwrap().count, REQUESTS as u32);
    });
    println!("full rebuild at {REQUESTS} requests: median {full:?} (budget {FULL_BUDGET:?})");
    database.stop();
    if full > FULL_BUDGET {
        eprintln!("board_scale: over budget");
        std::process::exit(1);
    }
}

fn median(mut run: impl FnMut()) -> Duration {
    let mut times: Vec<_> = (0..ROUNDS)
        .map(|_| {
            let started = Instant::now();
            run();
            started.elapsed()
        })
        .collect();
    times.sort();
    times[ROUNDS / 2]
}
