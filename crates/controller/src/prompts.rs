//! Built-in role prompts (PRD #327, C5). tl3d seeds them as versions when it
//! starts, so a deploy that changes one adds a version without undoing Rob's.

use serde_json::{Value, json};
use tl3_store::Store;

include!(concat!(env!("OUT_DIR"), "/worker_prompts.rs"));

/// Each worker role and the prompt this build ships for it, from
/// `workers/src/tl3_workers/prompts/<role>.md`.
pub const BUILTIN: &[(&str, &str)] = PACKAGED_PROMPTS;

pub fn seed(store: &mut Store) -> crate::Result<()> {
    Ok(store.seed_prompts(BUILTIN)?)
}

/// The request's `prompt`: the version this run uses, recorded with it.
/// Every worker role has a built-in prompt, and tl3d seeds them all at start,
/// so a role without an active version is a TL3 fault, never a reason to fall
/// back to the packaged file.
pub(crate) fn for_run(store: &mut Store, run_key: &str, role: &str) -> crate::Result<Value> {
    match store.run_prompt(run_key, role)? {
        Some(version) => Ok(json!({"version": version.id, "text": version.body})),
        None => {
            eprintln!("prompt store: worker role {role} has no active prompt version");
            Err(crate::Error::Invalid(
                "a worker role has no active prompt version; tl3d seeds every built-in prompt when it starts",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::Path};
    use tl3_store::Store;

    #[test]
    fn a_role_without_an_active_prompt_fails_the_run_instead_of_falling_back() {
        let mut database = crate::store_support::Database::start();
        let settings = database.create("prompts_without_pointer");
        let mut store = Store::open(&settings).unwrap();
        let error = super::for_run(&mut store, "effect:0:1", "ingest").unwrap_err();
        assert!(
            error.to_string().contains("no active prompt version"),
            "{error}"
        );
        super::seed(&mut store).unwrap();
        let shipped = super::BUILTIN
            .iter()
            .find(|(role, _)| *role == "ingest")
            .unwrap()
            .1;
        let prompt = super::for_run(&mut store, "effect:0:1", "ingest").unwrap();
        assert_eq!(prompt["text"], shipped);
        assert!(prompt["version"].as_i64().is_some_and(|id| id > 0));
        database.stop();
    }

    #[test]
    fn every_packaged_prompt_is_built_in() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers/src/tl3_workers/prompts");
        let packaged: BTreeSet<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter_map(|name| name.strip_suffix(".md").map(str::to_owned))
            .collect();
        let built_in: BTreeSet<String> = super::BUILTIN
            .iter()
            .map(|(role, _)| (*role).to_owned())
            .collect();
        assert_eq!(built_in, packaged);
    }
}
