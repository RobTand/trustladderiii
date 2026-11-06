//! Generate or check every contract at the aggregate dependency owner.
use std::{env, fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut check = false;
    let mut root = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--root" => root = Some(PathBuf::from(args.next().ok_or("--root needs a directory")?)),
            _ => return Err("usage: contract-schemas [--check] [--root DIR]".into()),
        }
    }
    let root = root.or_else(|| env::var_os("BUILD_WORKSPACE_DIRECTORY").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let mut stale = vec![];
    for file in tl3_controller::contract_schemas::generated() {
        let path = root.join(&file.path);
        if check { if fs::read_to_string(&path).ok().as_deref() != Some(&file.text) { stale.push(file.path); } }
        else { fs::create_dir_all(path.parent().ok_or("artifact needs a directory")?)?; fs::write(path, file.text)?; }
    }
    if !stale.is_empty() { return Err(format!("stale generated contracts: {}", stale.join(", ")).into()); }
    Ok(())
}
