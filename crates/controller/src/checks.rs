//! Backend-neutral check plans, staging and report judgement. Flux is the only executor;
//! these types and rules are shared by the Flux checks adapter and the controller.
use crate::{
    Error, Result,
    contracts::{CheckRequest, bounded::CommitSha},
    executor::Delivery,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
use tl3_worktrees::{CommitId, Worktrees};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRun {
    pub repository: PathBuf,
    pub base_sha: CommitId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_sha: Option<CommitId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproducing_unavailable: Option<String>,
    pub plan: Vec<Check>,
    /// Controller-owned membership of selected units that failed in any earlier round.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_failures: Vec<String>,
}

pub use crate::contracts::{Check, CheckKind as Kind};

pub(crate) mod results;
#[cfg(test)]
pub(crate) use results::contract_vectors;
pub(crate) use results::{HARNESS, selected_failures};

const MAX_REPORT_LINE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn validate_plan_size(plan: &[Check]) -> Result<()> {
    const {
        assert!(MAX_REPORT_LINE_BYTES as u64 == tl3_harness_core::limits::MAX_REPORT_LINE_BYTES);
        assert!(results::MAX_RESULT_BYTES as u64 == tl3_harness_core::limits::MAX_RESULT_BYTES);
        assert!(
            (65536
                + crate::findings::MAX_FAILED_TESTS * crate::findings::MAX_MESSAGE_CHARS * 12
                + 2) as u64
                == tl3_harness_core::limits::INITIAL_RESULT_BYTES
        );
    }
    use tl3_harness_core::limits::{
        add_cost, check_cost, measured_bytes, plan_line_fits, result_fits, test_cost,
    };
    let tests: BTreeSet<_> = plan.iter().flat_map(|check| &check.tests).collect();
    let collection = serde_json::to_vec(&serde_json::json!({"event":"collection", "tests":tests}))?;
    // Preserve this caller's UTF-8 collection and compact ASCII result costs.
    let collection_bytes = add_cost(
        measured_bytes(collection.len() as u128),
        add_cost(4, measured_bytes(tests.len().saturating_sub(1) as u128)),
    );
    let largest_phase = tests.iter().try_fold(0, |largest: u64, test| {
        serde_json::to_vec(test)
            .map(|bytes| largest.max(add_cost(measured_bytes(bytes.len() as u128), 8192)))
    })?;
    if !plan_line_fits(collection_bytes, largest_phase) {
        return Err(Error::Invalid(
            "Check plan exceeds the 4194304-byte report limit; reduce the selected tests or parameter cases.",
        ));
    }
    let mut result_bytes = tl3_harness_core::limits::INITIAL_RESULT_BYTES;
    for check in plan {
        let mut header = serde_json::json!({
            "name":check.name, "kind":check.kind, "custody":"in-process",
            "verdict":"infrastructure-failure", "tests":[],
        });
        if let Some(forcing) = &check.forcing {
            header["forcing"] = serde_json::to_value(forcing)?;
        }
        let proof_bytes = if matches!(check.kind, Kind::Demonstrating | Kind::Reproducing) {
            Some(if let Some(proof) = &check.proof {
                measured_bytes(ascii_json_bytes(&serde_json::to_value(proof)?)? as u128)
            } else {
                0
            })
        } else {
            None
        };
        if let Some(symptom) = &check.declared_symptom {
            header["declared_symptom"] = serde_json::to_value(symptom)?;
        }
        result_bytes = add_cost(
            result_bytes,
            check_cost(
                measured_bytes(ascii_json_bytes(&header)? as u128),
                proof_bytes,
                check.declared_symptom.is_some(),
            ),
        );
        for test in &check.tests {
            let mut case = serde_json::json!({
                "test":test, "base":"uncollected", "head":"uncollected",
                "pre_existing":false, "passed":false, "message":"",
            });
            if check.forcing.is_some() {
                case["head_forced"] = "uncollected".into();
            }
            let case_bytes = measured_bytes(ascii_json_bytes(&case)? as u128);
            let node_bytes = measured_bytes(ascii_json_bytes(&serde_json::json!(test))? as u128);
            result_bytes = add_cost(
                result_bytes,
                test_cost(case_bytes, node_bytes, check.forcing.is_some()),
            );
        }
        if !result_fits(result_bytes) {
            return Err(Error::Invalid(
                "Check plan exceeds the 8388608-byte result limit; reduce the selected tests or checks.",
            ));
        }
    }
    Ok(())
}

fn ascii_json_bytes(value: &serde_json::Value) -> Result<usize> {
    Ok(serde_json::to_string(value)?
        .chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 6 * ch.len_utf16() })
        .sum())
}

/// The plan with its base moved forward to the checkout's recorded base when the
/// head descends from it. A plan can keep an older base, as a restarted request
/// once inherited its predecessor's; checks must compare the head with the base
/// it actually sits on (#326). Rebases move the recorded base with the head.
pub(crate) fn descended_base(delivery: &Delivery) -> Result<CheckRun> {
    let mut run = delivery
        .checks
        .clone()
        .ok_or(Error::Invalid("missing check plan"))?;
    let Some(head) = delivery.head_sha.as_deref() else {
        return Ok(run);
    };
    let head = CommitId::new(head)?;
    let checkout = Worktrees::open(&run.repository, &run.repository)?;
    if let Some(base) = checkout.checked_out_base()?
        && base != run.base_sha
        && checkout.is_ancestor(&run.base_sha, &base)?
        && checkout.is_ancestor(&base, &head)?
    {
        run.base_sha = base;
    }
    Ok(run)
}

pub(crate) fn stage(
    root: &Path,
    harness: &Path,
    delivery: &Delivery,
    image: &str,
    protected_paths: &[String],
    tests: &crate::adapters::images::AdditionalTests,
) -> Result<Vec<String>> {
    let run = delivery
        .checks
        .as_ref()
        .ok_or(Error::Invalid("missing recorded check plan and base"))?;
    let head = delivery
        .head_sha
        .as_deref()
        .ok_or(Error::Invalid("checks require an exact head"))?;
    let head = CommitId::new(head)?;
    let base = &run.base_sha;
    let reviewed = run.reviewed_sha.as_ref();
    if (run.plan.iter().any(|c| c.kind == Kind::Reproducing) && reviewed.is_none())
        || !run.repository.is_absolute()
        || run.plan.is_empty()
        || run
            .plan
            .iter()
            .any(|c| c.name.is_empty() || c.tests.is_empty())
    {
        return Err(Error::Invalid("invalid check plan, repository or commit"));
    }
    if harness
        .canonicalize()?
        .starts_with(run.repository.canonicalize()?)
    {
        return Err(Error::Invalid(
            "install the trusted harness outside the served repository",
        ));
    }
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    fs::create_dir_all(root)?;
    // Never snapshot the maker's changing worktree. Clone committed objects into our private checkout.
    let repository = Worktrees::open(&run.repository, &run.repository)?;
    let served = root.join("served");
    let checkout = repository.clone_checkout(&served, &head)?;
    // The adopted head can diverge from its durable main base.
    checkout.import_commit(&run.repository, base)?;
    if checkout.verify_commit(base)? != *base {
        return Err(Error::Invalid("checks require an exact base"));
    }
    let changed = checkout
        .changed_paths_since_merge_base(base, &head)?
        .into_iter()
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| Error::Invalid("changed path must be UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    checkout.bundle_check_input(&root.join("served.bundle"), base, reviewed)?;
    fs::remove_dir_all(served)?;
    let trusted = root.join("trusted/tl3_harness");
    fs::create_dir_all(&trusted)?;
    for entry in fs::read_dir(harness)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|v| v == "py") && entry.file_type()?.is_file() {
            fs::copy(entry.path(), trusted.join(entry.file_name()))?;
        }
    }
    fs::write(root.join("action.py"), include_str!("checks/action.py"))?;
    let commit = |id: &CommitId| {
        CommitSha::new(id.as_str()).map_err(|_| Error::Invalid("checks require an exact commit"))
    };
    let request = CheckRequest {
        effect_key: delivery.key.clone(),
        base: commit(&run.base_sha)?,
        head: commit(&head)?,
        reviewed: run.reviewed_sha.as_ref().map(commit).transpose()?,
        plan: run.plan.clone(),
        image: image.to_owned(),
        protected_paths: protected_paths.to_vec(),
        footprint: delivery.paths.clone(),
        generated_paths: tests.generated.clone(),
        selected_failures: run.selected_failures.clone(),
        always_tests: tests.always.clone(),
        selected_tests: tests.selected,
    };
    // Through a Value, so the keys stay sorted and the staged bytes stay as they were.
    let request = serde_json::to_value(request)?;
    fs::write(root.join("request.json"), serde_json::to_vec(&request)?)?;
    Worktrees::commit_check_input(root)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_plan_base_rejects_uppercase_at_the_edge() {
        for length in [40, 64] {
            let mut value =
                serde_json::to_value(crate::git_tests::Repo::new().delivery().checks.unwrap())
                    .unwrap();
            value["base_sha"] = serde_json::json!("A".repeat(length));
            assert!(
                serde_json::from_value::<CheckRun>(value).is_err(),
                "reject uppercase check base before staging"
            );
        }
    }

    #[test]
    fn check_plan_reviewed_rejects_uppercase_at_the_edge() {
        for length in [40, 64] {
            let mut value =
                serde_json::to_value(crate::git_tests::Repo::new().delivery().checks.unwrap())
                    .unwrap();
            value["reviewed_sha"] = serde_json::json!("A".repeat(length));
            assert!(
                serde_json::from_value::<CheckRun>(value).is_err(),
                "reject uppercase reviewed commit before staging"
            );
        }
    }

    #[test]
    fn hostile_check_stage() {
        crate::git_tests::hostile(
            concat!(module_path!(), "::hostile_check_stage").trim_start_matches("tl3_controller::"),
            || {
                let f = crate::git_tests::Repo::new();
                let staged = f.root.path().join("input");
                let harness = f.root.path().join("harness");
                fs::create_dir(&harness).unwrap();
                fs::write(harness.join("__init__.py"), "").unwrap();
                let mut delivery = f.delivery();
                delivery.checks.as_mut().unwrap().selected_failures =
                    vec!["tests/test_old.py".into()];
                let changed = stage(
                    &staged,
                    &harness,
                    &delivery,
                    "sha256:fixture",
                    &[],
                    &Default::default(),
                )
                .unwrap();
                assert_eq!(changed, ["README.md"]);
                assert!(staged.join("served.bundle").is_file());
                let request: serde_json::Value =
                    serde_json::from_slice(&fs::read(staged.join("request.json")).unwrap())
                        .unwrap();
                assert_eq!(
                    request["selected_failures"],
                    serde_json::json!(["tests/test_old.py"])
                );
                assert_eq!(
                    crate::git_tests::git(&staged, &["show", "HEAD:request.json"]),
                    fs::read_to_string(staged.join("request.json")).unwrap()
                );
            },
        );
    }
}
