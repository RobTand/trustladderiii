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

/// Select the only worker tests executor, Flux, with fixture-only settings (PrismaBuild is retired).
pub fn select_flux(config: &mut tl3_config::Config, root: &std::path::Path) {
    let example = include_str!("../../../config/example.toml");
    let fixture = tl3_config::Config::parse(&select_flux_text(example, root)).unwrap();
    config.execution_backend = fixture.execution_backend;
    config.flux = fixture.flux;
}

/// Select Flux in a TOML config text built from the example, with a fixture `[flux]` table.
pub fn select_flux_text(text: &str, root: &std::path::Path) -> String {
    let selected = text.replacen(
        "# execution_backend = \"flux\"",
        "execution_backend = \"flux\"",
        1,
    );
    assert_ne!(
        selected, text,
        "example config lost its execution_backend line"
    );
    selected
        + &format!(
            "\n[flux]\ninstance_generation = \"fixture/00000000000000000000000000000000\"\n\
             executable = \"/fixture/flux\"\nbackend_cli = \"/fixture/tl3-run-backend\"\n\
             container_mode = \"own_scope\"\nuri = \"local:///fixture/flux\"\n\
             staging_root = {:?}\noutput_root = {:?}\ncontainer_image = \"sha256:{}\"\n",
            root.join("staging"),
            root.join("outputs"),
            "0".repeat(64),
        )
}
