//! Q51 (Rob, 2026-11-01): the first start under a new release clears each
//! open TL3-side incident once, so the fix a deploy carries gets its retry.

use crate::{Controller, Result, looks, put};
use serde_json::json;
use sha2::{Digest, Sha256};
use tl3_store::Commit;

const TABLE: &str = "releases";
const ROW: &str = "tl3d";

/// This build's release identity: the Git tree it was built from, or else the
/// digest of the running binary.
pub fn id() -> String {
    if let Some(tree) = option_env!("TL3_RELEASE_TREE") {
        return tree.into();
    }
    match std::env::current_exe().and_then(std::fs::read) {
        Ok(binary) => format!("sha256:{:x}", Sha256::digest(binary)),
        Err(error) => {
            eprintln!("release identity unavailable: {error}");
            String::new()
        }
    }
}

impl Controller {
    /// Record the release tl3d starts with. On a new release, clear each open
    /// TL3-side incident that holds work, through Rob's clear path. Its
    /// operation then retries; failing again raises a new incident.
    pub fn start_release(&mut self, release: &str) -> Result<()> {
        if release.is_empty() {
            return Ok(());
        }
        let snapshot = self.store.load()?;
        if snapshot
            .rows
            .iter()
            .any(|row| row.table == TABLE && row.id == ROW && row.value["id"] == release)
        {
            return Ok(());
        }
        let mut commit = Commit::new(
            format!("release:{release}:{}", snapshot.revision.get()),
            json!({"release": release}),
            &snapshot,
        );
        let mut cleared = Vec::new();
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "incidents" && row.value["open"] == true)
        {
            let incident: looks::Incident = crate::decode_stored(row.value.clone())?;
            if incident.cause() != looks::Cause::Tl3
                || !(incident.blocking || incident.effect.is_some())
            {
                continue;
            }
            cleared.push(incident.id.clone());
            let inbox = format!("incident-release:{release}:{}", incident.id);
            self.clear_incident(&snapshot, incident, inbox, &mut commit)?;
        }
        commit
            .rows
            .push(put(TABLE, ROW.into(), json!({"id": release})));
        self.store.commit(&commit)?;
        eprintln!(
            "release {release}: cleared {} TL3-side incidents {cleared:?}",
            cleared.len()
        );
        Ok(())
    }
}
