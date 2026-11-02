//! Built-in role prompts (PRD #327, C5). tl3d seeds them as versions when it
//! starts, so a deploy that changes one adds a version without undoing Rob's.

use serde_json::{Value, json};
use tl3_store::Store;

macro_rules! builtin {
    ($($role:literal),* $(,)?) => {
        &[$(($role, include_str!(concat!("../../../workers/src/tl3_workers/prompts/", $role, ".md")))),*]
    };
}

/// Each worker role and the prompt this build ships for it.
pub const BUILTIN: &[(&str, &str)] = builtin!(
    "audit",
    "classifier",
    "ingest",
    "maker",
    "planner",
    "reviewer",
    "writer"
);

pub fn seed(store: &mut Store) -> crate::Result<()> {
    Ok(store.seed_prompts(BUILTIN)?)
}

/// The request's `prompt`: the version this run uses, recorded with it. None
/// leaves the worker on its packaged prompt.
pub(crate) fn for_run(
    store: &mut Store,
    run_key: &str,
    role: &str,
) -> crate::Result<Option<Value>> {
    Ok(store
        .run_prompt(run_key, role)?
        .map(|version| json!({"version": version.id, "text": version.body})))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::Path};

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
