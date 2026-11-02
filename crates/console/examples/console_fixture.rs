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
use tl3_controller::{Bootstrap, Controller, LabelledIssue, Repository, adapters, board};
use tl3_kernel::{Command, Event, NodeId};
use tl3_store::{Reader, Store};

const TITLES: [&str; 8] = [
    "Shard the GPU benchmark queue by device class",
    "Retry Hindsight writes after a 429",
    "Quantize attention layers to FP8 with calibration",
    "Fix tile seams at zoom level 14",
    "Document the merge-queue batching rules",
    "Cache compiled kernels between campaign rows",
    "Remove the stale celestia worker from the fleet file",
    "Report per-shard wall time in the CI summary",
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
    for (index, title) in TITLES.iter().enumerate() {
        controller
            .intake(issue(repos[index % repos.len()], 100 + index as u64, title))
            .unwrap();
    }
    settle(&mut controller);
    for (node, command) in [(NodeId(1), Command::Pause), (NodeId(2), Command::Stop)] {
        controller
            .observe(
                &format!("fixture-{}", node.0),
                tl3_controller::executor::Observation::Kernel(Event::Command {
                    node,
                    id: 1,
                    command,
                }),
            )
            .unwrap();
    }
    settle(&mut controller);
    controller
        .intake(issue("tl3-sandbox", 42, "Add a live board to the operator console"))
        .unwrap();
    let console = tl3_console::start(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        tl3_console::Options {
            store: settings.clone(),
            assets,
            models: None,
        },
    )
    .unwrap();
    println!("{}", json!({"url": format!("http://{}", console.address())}));
    for line in std::io::stdin().lock().lines() {
        if line.unwrap().trim() != "advance" {
            continue;
        }
        let before = column(&settings);
        let mut after = before;
        for _ in 0..100 {
            if after != before || !controller.drive_once().unwrap() {
                break;
            }
            after = column(&settings);
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

fn settle(controller: &mut Controller) {
    for _ in 0..500 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
}

/// Request #42's column, read the way the console reads it.
fn column(settings: &tl3_config::Store) -> board::Column {
    let mut reader = Reader::open(settings).unwrap();
    let mut tx = reader.transaction().unwrap();
    let board = board::read(&mut tx, 0, None).unwrap();
    tx.commit().unwrap();
    board
        .cards
        .iter()
        .find(|card| card.issue == 42 && card.repo == "RobTand/tl3-sandbox")
        .map(|card| card.column)
        .unwrap()
}
