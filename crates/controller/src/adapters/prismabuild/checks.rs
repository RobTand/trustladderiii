use crate::{Error, Result, executor::Delivery};
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub kind: Kind,
    pub tests: Vec<String>,
    /// A demonstrating check's forced reproduction (#393). The plan's declarative forcing,
    /// passed through untouched: the trusted harness bounds it and applies it in the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forcing: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_evidence: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_symptom: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Demonstrating,
    Regression,
    BehaviorPreserving,
    Reproducing,
}

const MAX_REPORT_LINE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn validate_plan_size(plan: &[Check]) -> Result<()> {
    let tests: BTreeSet<_> = plan.iter().flat_map(|check| &check.tests).collect();
    let collection = serde_json::to_vec(&serde_json::json!({"event":"collection", "tests":tests}))?;
    // Python's event encoding adds three object spaces, list separators and a newline.
    let collection_bytes = collection.len() + 4 + tests.len().saturating_sub(1);
    let oversized_phase = tests.iter().try_fold(false, |oversized, test| {
        serde_json::to_vec(test)
            .map(|bytes| oversized || bytes.len() + 8192 > MAX_REPORT_LINE_BYTES)
    })?;
    if collection_bytes > MAX_REPORT_LINE_BYTES || oversized_phase {
        return Err(Error::Invalid(
            "Check plan exceeds the 4194304-byte report limit; reduce the selected tests or parameter cases.",
        ));
    }
    let mut result_bytes =
        65536 + crate::findings::MAX_FAILED_TESTS * crate::findings::MAX_MESSAGE_CHARS * 12 + 2;
    for check in plan {
        let mut header = serde_json::json!({
            "name":check.name, "kind":check.kind, "custody":"in-process",
            "verdict":"infrastructure-failure", "tests":[],
        });
        if let Some(forcing) = &check.forcing {
            // A forced check's result echoes its forcing, and each test gains head_forced (#393).
            header["forcing"] = forcing.clone();
        }
        if let Some(evidence) = &check.helper_evidence {
            let observers: std::collections::BTreeMap<_, _> =
                check.tests.iter().map(|t| (t, "0".repeat(64))).collect();
            header["helper_evidence"] = serde_json::json!({"target": evidence["target"],
                "base_target_sha256":"0".repeat(64), "head_target_sha256":"0".repeat(64),
                "base_tree_sha256":"0".repeat(64), "head_tree_sha256":"0".repeat(64), "observers":observers,
                "failure_cases":evidence["failure_cases"], "failure_driver_sha256":"0".repeat(64),
                "failure_preservation":evidence["failure_cases"].as_array().ok_or(Error::Invalid("helper evidence requires failure cases"))?.iter().map(|_| serde_json::json!({
                    "base":"infrastructure-failure", "head":"infrastructure-failure"})).collect::<Vec<_>>()});
        }
        if let Some(symptom) = &check.declared_symptom {
            header["declared_symptom"] = symptom.clone();
            // Up to thirty reports, each with a bounded pytest traceback and identity.
            result_bytes += 384 * 1024;
        }
        result_bytes += ascii_json_bytes(&header)? + 1;
        for test in &check.tests {
            let mut case = serde_json::json!({
                "test":test, "base":"uncollected", "head":"uncollected",
                "pre_existing":false, "passed":false, "message":"",
            });
            if check.forcing.is_some() {
                case["head_forced"] = "uncollected".into();
            }
            result_bytes += ascii_json_bytes(&case)? + 1;
            // Match the harness reservation: three bounded phase facts and the concrete node per side.
            let sides = if check.forcing.is_some() { 3 } else { 2 };
            result_bytes +=
                sides * (3 * (200 * 12 + 100) + 100 + ascii_json_bytes(&serde_json::json!(test))?);
        }
        if result_bytes > super::results::MAX_RESULT_BYTES {
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
pub(super) fn descended_base(delivery: &Delivery) -> Result<CheckRun> {
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

pub(super) fn stage(
    root: &Path,
    harness: &Path,
    delivery: &Delivery,
    image: &str,
    protected_paths: &[String],
    tests: &super::super::images::AdditionalTests,
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
    fs::write(root.join("action.py"), include_str!("action.py"))?;
    let mut request = serde_json::json!({
        "effect_key": delivery.key, "base": run.base_sha, "head": head.as_str(),
        "reviewed": run.reviewed_sha,
        "plan": run.plan, "image": image,
        "protected_paths": protected_paths,
    });
    request["selected_failures"] = serde_json::json!(run.selected_failures);
    request["always_tests"] = serde_json::json!(tests.always);
    if let Some(limit) = tests.selected {
        request["selected_tests"] = limit.into();
    }
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
                    false,
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
