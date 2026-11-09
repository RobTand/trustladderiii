//! Bounded private JSON-line transport for the Rust action policy.
use std::io::{self, BufRead, Read, Write};
use std::process::ExitCode;
const REQUEST_BYTES: u64 = 64 * 1024 * 1024;
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() == ["schemas"] {
        for file in tl3_action::generated() {
            let path = std::path::Path::new(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, file.text)?;
        }
        return Ok(());
    }
    if args.as_slice() != ["policy"] {
        return Err("usage: tl3-action policy | schemas".into());
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = String::new();
        if input
            .by_ref()
            .take(REQUEST_BYTES + 1)
            .read_line(&mut line)?
            == 0
        {
            break;
        }
        if line.len() as u64 > REQUEST_BYTES || !line.ends_with('\n') {
            return Err("action request exceeds private transport bound or is incomplete".into());
        }
        let response = match serde_json::from_str::<tl3_action::PolicyRequest>(&line) {
            Ok(request) => match tl3_action::policy(request) {
                Ok(response) => serde_json::json!({"ok":response}),
                Err(_) => serde_json::json!({"error":"Rust action rejected invalid policy input"}),
            },
            Err(_) => serde_json::json!({"error":"Rust action rejected malformed policy request"}),
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
