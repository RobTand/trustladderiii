#![allow(dead_code)]
#[path = "../../../store/tests/support/mod.rs"]
mod database;
pub use database::Database;
pub mod controller_restart;
pub mod executor_probe;
pub mod github;
pub mod scenario;
pub mod scenario_shard;
pub mod workers;

/// Select the only worker tests executor, Flux, with the example's offline placeholder
/// settings (PrismaBuild is retired). Validation is lexical; nothing here touches Flux.
pub fn select_flux(config: &mut tl3_config::Config) {
    let example = include_str!("../../../config/example.toml");
    let fixture = tl3_config::Config::parse(&select_flux_text(example)).unwrap();
    config.execution_backend = fixture.execution_backend;
    config.flux = fixture.flux;
}

/// Select Flux in a TOML config text built from the example by enabling its commented
/// `execution_backend` line and `[flux]` placeholder table.
pub fn select_flux_text(text: &str) -> String {
    let mut in_flux = false;
    let mut selected = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.starts_with("# execution_backend = \"flux\"") || line.starts_with("# [flux]") {
            in_flux = line.starts_with("# [flux]");
            selected.push_str(&line[2..]);
        } else if in_flux && line.starts_with("# ") && line[2..].contains(" = ") {
            selected.push_str(&line[2..]);
        } else {
            in_flux = false;
            selected.push_str(line);
        }
    }
    assert_ne!(selected, text, "example config lost its Flux placeholders");
    selected
}
