//! Write the JSON Schemas generated from `tl3_controller::contracts` (#741).
//!
//! tl3-contracts-schemas [--check] [--root DIR]
//!
//! `--check` writes nothing and exits 1 naming every stale file. The repository root is
//! `--root`, else Bazel's `BUILD_WORKSPACE_DIRECTORY` under `bazel run`, else this
//! crate's checkout.

use std::{env, fs, path::PathBuf, process::ExitCode};

const FIX: &str = "bazel run --config=remote //crates/controller:tl3-contracts-schemas";

fn main() -> ExitCode {
    let mut check = false;
    let mut root = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--root" => root = args.next().map(PathBuf::from),
            _ => {
                eprintln!("usage: tl3-contracts-schemas [--check] [--root DIR]");
                return ExitCode::from(2);
            }
        }
    }
    let root = root
        .or_else(|| env::var_os("BUILD_WORKSPACE_DIRECTORY").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let mut stale = vec![];
    for file in tl3_controller::contracts::generated() {
        let path = root.join(&file.path);
        if fs::read_to_string(&path).ok().as_deref() == Some(file.text.as_str()) {
            continue;
        }
        if check {
            stale.push(file.path);
        } else if let Err(error) = fs::write(&path, &file.text) {
            eprintln!("{}: {error}", path.display());
            return ExitCode::FAILURE;
        } else {
            println!("wrote {}", file.path);
        }
    }
    if stale.is_empty() {
        return ExitCode::SUCCESS;
    }
    for path in stale {
        eprintln!("{path} is stale: it no longer matches the Rust contract types");
    }
    eprintln!("Regenerate with: {FIX}");
    ExitCode::FAILURE
}
