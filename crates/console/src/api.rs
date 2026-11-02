//! The console's HTTP API types. The web app's TypeScript types are generated
//! from these and the board's, so the two can't drift.

use serde::Serialize;
use tl3_controller::board::{
    Badge, Board, BoardPatch, Card, CardIncident, Column, ColumnCount, Ending, Flow, Retries,
};
use ts_rs::TS;

/// The stream's `database` event: whether the console can read the store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct DatabaseStatus {
    pub connected: bool,
}

/// The stream's `patch` event: apply `patch` to the board whose event id is
/// `base`. A client holding another board reconnects for a fresh snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct PatchEvent {
    pub base: String,
    pub patch: BoardPatch,
}

/// The generated `web/src/api/types.gen.ts`.
pub fn typescript() -> String {
    let config = ts_rs::Config::new().with_large_int("number");
    let mut output = String::from(
        "// Generated from the console's Rust types by crates/console/src/api.rs. Do not edit.\n",
    );
    for (docs, decl) in [
        (Board::docs(), Board::decl(&config)),
        (BoardPatch::docs(), BoardPatch::decl(&config)),
        (Card::docs(), Card::decl(&config)),
        (CardIncident::docs(), CardIncident::decl(&config)),
        (Column::docs(), Column::decl(&config)),
        (ColumnCount::docs(), ColumnCount::decl(&config)),
        (Badge::docs(), Badge::decl(&config)),
        (Ending::docs(), Ending::decl(&config)),
        (Flow::docs(), Flow::decl(&config)),
        (Retries::docs(), Retries::decl(&config)),
        (DatabaseStatus::docs(), DatabaseStatus::decl(&config)),
        (PatchEvent::docs(), PatchEvent::decl(&config)),
    ] {
        output.push('\n');
        output.push_str(&docs.unwrap_or_default());
        output.push_str("export ");
        output.push_str(&decl);
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::Path};

    #[test]
    fn web_api_types_are_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/api/types.gen.ts");
        let generated = super::typescript();
        if env::var_os("TL3_WRITE_API_TYPES").is_some() {
            fs::write(&path, &generated).unwrap();
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default(),
            generated,
            "web/src/api/types.gen.ts is stale; regenerate it with TL3_WRITE_API_TYPES=1 cargo test -p tl3-console --lib"
        );
    }
}
