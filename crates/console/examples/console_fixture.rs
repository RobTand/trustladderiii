//! The web app's end-to-end fixture: an embedded Postgres, a store seeded by
//! the simulated controller, and the console serving built web assets.
//!
//! Prints `{"url": ...}` once serving. Each `advance` line on stdin drives the
//! controller until request #42 changes column and prints `{"column": ...}`.
//! Closing stdin stops Postgres and exits.

#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{io::BufRead, net::TcpListener, path::PathBuf};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    board::{self, Column},
    executor::Observation,
};
use tl3_kernel::{Command, Event, NodeId};
use tl3_store::{Reader, Store};

const REPOS: [&str; 4] = ["tl3-sandbox", "prismabuild", "prismaquant", "tessera"];

/// Seeded requests and how far each is driven before the next arrives.
const SEED: [(&str, Column); 8] = [
    (
        "Shard the GPU benchmark queue by device class",
        Column::Done,
    ),
    ("Retry Hindsight writes after a 429", Column::Done),
    (
        "Quantize attention layers to FP8 with calibration",
        Column::Merging,
    ),
    ("Fix tile seams at zoom level 14", Column::Reviewing),
    ("Document the merge-queue batching rules", Column::Checking),
    (
        "Cache compiled kernels between campaign rows",
        Column::Making,
    ),
    (
        "Remove the stale celestia worker from the fleet file",
        Column::Planning,
    ),
    (
        "Report per-shard wall time in the CI summary",
        Column::Intake,
    ),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let assets = match (args.next().as_deref(), args.next()) {
        (Some("--assets"), Some(path)) => PathBuf::from(path),
        _ => panic!("usage: console_fixture --assets DIR"),
    };
    let mut database = support::Database::start();
    let settings = database.create("console_fixture");
    let records = tempfile::tempdir().unwrap();
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(records.path()).unwrap(),
        Bootstrap {
            operations: None,
            repos: REPOS
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
    for (index, (title, target)) in SEED.into_iter().enumerate() {
        let (repo, number) = (REPOS[index % REPOS.len()], 100 + index as u64);
        controller.intake(issue(repo, number, title)).unwrap();
        while column(&settings, repo, number) < target && controller.drive_once().unwrap() {}
    }
    for (node, command) in [(NodeId(5), Command::Pause), (NodeId(4), Command::Stop)] {
        let event = Event::Command {
            node,
            id: 1,
            command,
        };
        controller
            .observe(&format!("fixture-{}", node.0), Observation::Kernel(event))
            .unwrap();
        controller.drive_once().unwrap();
    }
    controller
        .intake(issue(
            "tl3-sandbox",
            42,
            "Add a live board to the operator console",
        ))
        .unwrap();
    let console = tl3_console::start(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        tl3_console::Options {
            store: settings.clone(),
            assets,
            models: Some(
                tl3_config::Config::parse(include_str!("../../config/example.toml"))
                    .unwrap()
                    .models,
            ),
        },
    )
    .unwrap();
    println!(
        "{}",
        json!({"url": format!("http://{}", console.address())})
    );
    for line in std::io::stdin().lock().lines() {
        if line.unwrap().trim() != "advance" {
            continue;
        }
        let before = column(&settings, "tl3-sandbox", 42);
        let mut after = before;
        while after == before && controller.drive_once().unwrap() {
            after = column(&settings, "tl3-sandbox", 42);
        }
        println!("{}", json!({ "column": after }));
    }
    database.stop();
}

fn issue(repo: &str, number: u64, title: &str) -> LabelledIssue {
    LabelledIssue {
        delivery: format!("fixture-{repo}-{number}"),
        owner: "RobTand".into(),
        repo: repo.into(),
        installation: 1,
        number,
        title: title.into(),
        actor: 24923402,
        label: "tl3".into(),
    }
}

/// A request's column, read the way the console reads it.
fn column(settings: &tl3_config::Store, repo: &str, issue: u64) -> Column {
    let mut reader = Reader::open(settings).unwrap();
    let mut tx = reader.transaction().unwrap();
    let board = board::read(&mut tx, 0, None).unwrap();
    tx.commit().unwrap();
    let repo = format!("RobTand/{repo}");
    board
        .cards
        .iter()
        .find(|card| card.issue == issue && card.repo == repo)
        .map(|card| card.column)
        .unwrap()
}
