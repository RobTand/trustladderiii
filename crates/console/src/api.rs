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

/// The generated `web/src/api/types.gen.ts`.
pub fn typescript() -> String {
    let _ = (
        std::marker::PhantomData::<(
            Badge,
            Board,
            BoardPatch,
            Card,
            CardIncident,
            Column,
            ColumnCount,
            Ending,
            Flow,
            Retries,
        )>,
    );
    todo!()
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
