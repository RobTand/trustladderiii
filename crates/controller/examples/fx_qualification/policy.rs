//! Pure qualification decisions. No scheduler or filesystem access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Unknown,
    NotRun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Canary {
    LaunchFailure,
    Cancel,
    Success,
    Generator,
}

pub const ORDER: [Canary; 4] = [
    Canary::LaunchFailure,
    Canary::Cancel,
    Canary::Success,
    Canary::Generator,
];

pub fn verdict(canaries: &[(Canary, Status)], pin_checked: bool) -> Status {
    if canaries.len() != ORDER.len()
        || canaries
            .iter()
            .zip(ORDER)
            .any(|((kind, _), expected)| *kind != expected)
    {
        return Status::Unknown;
    }
    if canaries.iter().any(|(_, status)| *status == Status::Fail) {
        Status::Fail
    } else if pin_checked && canaries.iter().all(|(_, status)| *status == Status::Pass) {
        Status::Pass
    } else {
        Status::Unknown
    }
}

pub const PIN: &str = "crates/controller/src/adapters/flux_checks/activation/qualification.json";
pub const INDEX: &str = "crates/controller/qualification/current.json";
pub const INPUTS: &str = "crates/controller/qualification/fx-current/";
pub const PATHSET_VERSION: &str = "executor-pathset-v1-amended-20261110";

// Explicit proof-only directories inspected at the X-3 base. New directories
// remain relevant until this policy is reviewed again.
const PROOFS: &[&str] = &[
    "crates/controller-core/verus/",
    "crates/harness-core/verus/",
    "crates/controller/proofs/",
    "crates/delivery/proofs/",
    "crates/github/proofs/",
    "crates/plan-check/proofs/",
    "crates/verus-runner/proofs/",
];

pub fn derived(path: &str) -> bool {
    path == PIN || path == INDEX || path.starts_with(INPUTS)
}

pub fn relevant(path: &str) -> bool {
    if derived(path)
        || path.starts_with("crates/kernel/")
        || path.starts_with("crates/verus-runner/")
        || PROOFS.iter().any(|prefix| path.starts_with(prefix))
        || path.starts_with("tools/pb_nightly/")
        || path.starts_with("tools/ci/verus_")
        || path.starts_with("tools/ci/kani_")
        || path.starts_with("docs/")
        || path.starts_with("tools/bazel/verus") && path.ends_with(".bzl")
        || path.ends_with(".md")
            && !path.starts_with("harness/")
            && !path.starts_with("crates/controller/qualification/")
    {
        return false;
    }
    [
        "crates/", "harness/", "workers/", "tools/", "deploy/", ".cargo/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        || [
            "Cargo.toml",
            "Cargo.lock",
            "BUILD.bazel",
            "MODULE.bazel",
            "MODULE.bazel.lock",
            ".bazelrc",
            ".bazelversion",
            ".bazelignore",
            ".gitignore",
            "gazelle_python.yaml",
        ]
        .contains(&path)
}

pub fn publication_order(paths: &[String]) -> Result<Vec<String>, &'static str> {
    let unique: std::collections::BTreeSet<_> = paths.iter().collect();
    if unique.len() != paths.len()
        || !paths.iter().all(|p| derived(p))
        || !paths.iter().any(|p| p == INDEX)
        || !paths.iter().any(|p| p == PIN)
        || paths.iter().any(|p| {
            p.split('/')
                .any(|part| part == ".." || part == "." || part.is_empty())
        })
    {
        return Err("qualification.publication.inventory");
    }
    let mut result = paths.to_vec();
    result.sort_by_key(|path| {
        (
            if path == INDEX {
                2
            } else if path == PIN {
                1
            } else {
                0
            },
            path.clone(),
        )
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted() -> Vec<(Canary, Status)> {
        ORDER.into_iter().map(|kind| (kind, Status::Pass)).collect()
    }

    #[test]
    fn refuses_every_non_pass_canary_and_unchecked_pin() {
        let fixture = accepted();
        assert_eq!(verdict(&fixture, true), Status::Pass);
        assert_eq!(verdict(&fixture, false), Status::Unknown);
        for index in 0..4 {
            for failure in [Status::Fail, Status::Unknown, Status::NotRun] {
                let mut candidate = fixture.clone();
                candidate[index].1 = failure;
                assert_ne!(verdict(&candidate, true), Status::Pass);
            }
        }
    }

    #[test]
    fn missing_duplicate_or_reordered_evidence_never_passes() {
        let mut fixture = accepted();
        fixture.pop();
        assert_ne!(verdict(&fixture, true), Status::Pass);
        fixture = accepted();
        fixture[0].0 = Canary::Success;
        assert_ne!(verdict(&fixture, true), Status::Pass);
        fixture = accepted();
        fixture.swap(0, 1);
        assert_ne!(verdict(&fixture, true), Status::Pass);
    }

    #[test]
    fn carry_distinguishes_kernel_proof_and_executor_changes() {
        for path in [
            "crates/kernel/src/lib.rs",
            "crates/controller-core/verus/new_proof.rs",
            "tools/bazel/verus.bzl",
            "tools/ci/kani_gate.py",
            "tools/ci/verus_measurement/summary.json",
            "tools/pb_nightly/entry.sh",
            "docs/architecture.md",
            "README.md",
            "crates/controller/qualification/fx-current/success.json",
            "crates/controller/qualification/current.json",
            "crates/controller/src/adapters/flux_checks/activation/qualification.json",
        ] {
            assert!(!relevant(path), "{path}");
        }
        for path in [
            "crates/controller/src/adapters/run_backend/flux/mod.rs",
            "crates/controller-core/src/lib.rs",
            "harness/src/tl3_harness/future_module.py",
            "harness/README.md",
            "workers/src/tl3_workers/generators.py",
            "tools/ci/flux_gate.py",
            "crates/controller/tools/generate_fx_qualification.rs",
            "crates/controller/qualification/README.md",
            "crates/controller/qualification/fx-exact42b/fixtures/host-collector",
            "Cargo.lock",
            ".cargo/config.toml",
            "deploy/dl380g10/build-release.sh",
        ] {
            assert!(relevant(path), "{path}");
        }
    }

    #[test]
    fn publication_writes_index_last_and_refuses_unrelated_paths() {
        let fixture: Vec<String> = [
            "crates/controller/qualification/current.json",
            "crates/controller/src/adapters/flux_checks/activation/qualification.json",
            "crates/controller/qualification/fx-current/success.json",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let ordered = publication_order(&fixture).unwrap();
        assert_eq!(ordered.last(), Some(&fixture[0]));
        assert_eq!(ordered.first(), Some(&fixture[2]));
        assert!(publication_order(&["harness/src/tl3_harness/runtime.py".into()]).is_err());
        assert!(publication_order(&[fixture[0].clone(), fixture[0].clone()]).is_err());
    }
}
