//! Aggregate inventory without moving controller/harness-owned wire types.
use tl3_controller::{budget, check_harness, quota};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var_os("BUILD_WORKSPACE_DIRECTORY").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let check = std::env::args().any(|arg| arg == "--check");
    for (name, schema) in [
        ("budget-wire", budget::wire_schema()),
        ("harness-wire", check_harness::wire_schema()),
        ("quota-event", serde_json::to_value(schemars::schema_for!(quota::QuotaEvent))?),
    ] {
        let path = root.join(format!("{}/{name}.json", tl3_contracts::SCHEMA_DIRECTORY));
        let text = format!("{}\n", serde_json::to_string_pretty(&schema)?);
        // Existing wire schemas have distinct published key layouts. Drift compares
        // their JSON shape; role reply schemas retain the stricter byte check.
        if check {
            let committed: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            if committed != schema { return Err(format!("{name} differs from its Rust owner").into()); }
        } else { std::fs::write(path, text)?; }
    }
    Ok(())
}
