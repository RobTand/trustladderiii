use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryEvidence {
    pub kind: String,
    pub job: Option<Value>,
    pub status: String,
    pub ending: Option<api::Ending>,
    pub terminal_receipt: Option<api::BlobRef>,
    pub cleanup: Option<api::CleanupResult>,
    pub executions: Vec<Value>,
    pub artifacts: Vec<Value>,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub pass: bool,
    pub evidence: Vec<api::BlobRef>,
}

pub fn decisions(canaries: &[CanaryEvidence]) -> Result<Vec<(policy::Canary, policy::Status)>> {
    canaries
        .iter()
        .map(|canary| {
            Ok((
                match canary.kind.as_str() {
                    "launch_failure" => policy::Canary::LaunchFailure,
                    "cancel" => policy::Canary::Cancel,
                    "success" => policy::Canary::Success,
                    "generator" => policy::Canary::Generator,
                    _ => return Err("unknown canary kind".into()),
                },
                match canary.status.as_str() {
                    "PASS" if canary.checks.iter().any(|check| !check.pass) => policy::Status::Fail,
                    "PASS" if accepted(canary) => policy::Status::Pass,
                    "FAIL" => policy::Status::Fail,
                    "NOT_RUN" => policy::Status::NotRun,
                    _ => policy::Status::Unknown,
                },
            ))
        })
        .collect()
}

fn accepted(canary: &CanaryEvidence) -> bool {
    let expected = match canary.kind.as_str() {
        "launch_failure" => api::Ending::InfraLost,
        "cancel" => api::Ending::Cancelled,
        _ => api::Ending::Passed,
    };
    let mut required = vec![
        "typed_ending",
        "clean_event",
        "verified_cleanup",
        "scratch_absence",
        "shape_allocation",
        "job_cgroup_absent",
        "instance_receipts_complete",
    ];
    match canary.kind.as_str() {
        "launch_failure" => required.push("post_begin_launch_failure"),
        "cancel" => required.push("owned_cancel_after_readiness"),
        "success" => required.push("real_pytest_base_red_head_green"),
        "generator" => required.extend([
            "bounded_generator_artifact",
            "target_flux_uri_broker_and_containment_probes",
            "actual_image_and_read_only_socket_mount",
        ]),
        _ => return false,
    }
    canary
        .job
        .as_ref()
        .and_then(|job| job["id"].as_str())
        .is_some_and(|id| !id.is_empty())
        && canary.ending.as_ref() == Some(&expected)
        && canary.terminal_receipt.is_some()
        && canary
            .cleanup
            .as_ref()
            .is_some_and(|cleanup| cleanup.stopped && cleanup.pending_executions.is_empty())
        && required.iter().all(|name| {
            canary
                .checks
                .iter()
                .filter(|check| check.name == *name && check.pass)
                .count()
                == 1
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verdict_assembly_refuses_pass_claim_without_receipts_or_checks() {
        let mut canary = CanaryEvidence {
            kind: "success".into(),
            job: None,
            status: "PASS".into(),
            ending: Some(api::Ending::Passed),
            terminal_receipt: None,
            cleanup: None,
            executions: vec![],
            artifacts: vec![],
            checks: vec![],
        };
        assert_eq!(
            decisions(&[canary.clone()]).unwrap()[0].1,
            policy::Status::Unknown
        );
        canary.checks.push(Check {
            name: "typed_ending".into(),
            pass: false,
            evidence: vec![],
        });
        assert_eq!(decisions(&[canary]).unwrap()[0].1, policy::Status::Fail);
    }

    #[test]
    fn carry_compares_git_blobs_and_modes_across_the_two_heads() {
        let root = tempfile::tempdir().unwrap();
        git(
            root.path(),
            &["init", "--quiet", "--template=", "-b", "main"],
        )
        .unwrap();
        for (name, bytes) in [
            ("Cargo.toml", "workspace"),
            ("crates/kernel/src/lib.rs", "proof"),
            ("harness/host-collector", "helper"),
        ] {
            let path = root.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        git(root.path(), &["add", "--all"]).unwrap();
        git(root.path(), &["commit", "--quiet", "-m", "qualified"]).unwrap();
        let qualified = git(root.path(), &["rev-parse", "HEAD"]).unwrap();
        fs::write(root.path().join("crates/kernel/src/lib.rs"), "new proof").unwrap();
        git(root.path(), &["add", "--all"]).unwrap();
        git(root.path(), &["commit", "--quiet", "-m", "kernel"]).unwrap();
        let kernel = git(root.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            pathset(root.path(), &qualified).unwrap(),
            pathset(root.path(), &kernel).unwrap()
        );
        fs::set_permissions(
            root.path().join("harness/host-collector"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        git(root.path(), &["add", "--all"]).unwrap();
        git(root.path(), &["commit", "--quiet", "-m", "helper mode"]).unwrap();
        let changed = git(root.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(
            pathset(root.path(), &qualified).unwrap(),
            pathset(root.path(), &changed).unwrap()
        );
    }
}

pub fn verify_references(value: &Value) -> Result<()> {
    match value {
        Value::Object(fields) => {
            if let (Some(path), Some(hash), Some(bytes)) = (
                fields.get("path").and_then(Value::as_str),
                fields.get("sha256").and_then(Value::as_str),
                fields.get("bytes").and_then(Value::as_u64),
            ) {
                let path = Path::new(path);
                if !api::absolute(path) || path.ancestors().any(Path::is_symlink) {
                    return Err("evidence path must be absolute and nonsymlink".into());
                }
                let observed = identity(path)?;
                if observed["sha256"] != hash || observed["bytes"] != bytes {
                    return Err("referenced evidence hash/size changed".into());
                }
                if let Some(mode) = fields.get("mode") {
                    if observed["mode"] != *mode {
                        return Err("referenced artifact mode changed".into());
                    }
                }
            }
            for child in fields.values() {
                verify_references(child)?;
            }
        }
        Value::Array(children) => {
            for child in children {
                verify_references(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn pathset(repo: &Path, head: &str) -> Result<BTreeMap<String, String>> {
    if !api::hex(head, 40) {
        return Err("full Git commit required".into());
    }
    let output = std::process::Command::new("/usr/bin/git")
        .current_dir(repo)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["ls-tree", "-rz", "--full-tree", head])
        .output()?;
    if !output.status.success() {
        return Err("Git tree inspection failed".into());
    }
    let mut entries = BTreeMap::new();
    for line in output
        .stdout
        .split(|c| *c == 0)
        .filter(|line| !line.is_empty())
    {
        let line = std::str::from_utf8(line)?;
        let (metadata, path) = line.split_once('\t').ok_or("invalid Git tree entry")?;
        if policy::relevant(path) {
            entries.insert(path.into(), metadata.into());
        }
    }
    if entries.is_empty() {
        return Err("empty executor pathset refused".into());
    }
    Ok(entries)
}

pub fn carry(args: &[std::ffi::OsString]) -> Result<()> {
    let mut fields = BTreeMap::new();
    for pair in args.chunks(2) {
        if pair.len() != 2 {
            return Err("carry requires --repo --verdict --head".into());
        }
        let name = pair[0].to_str().ok_or("UTF-8 option required")?;
        if !["--repo", "--verdict", "--head"].contains(&name)
            || fields
                .insert(name, pair[1].to_str().ok_or("UTF-8 input required")?)
                .is_some()
        {
            return Err("invalid carry option".into());
        }
    }
    let get = |name| fields.get(name).copied().ok_or("missing carry option");
    let repo = Path::new(get("--repo")?);
    let path = Path::new(get("--verdict")?);
    let head = get("--head")?;
    let mut reasons = Vec::new();
    let mut changed = Vec::new();
    let result = (|| -> Result<()> {
        let verdict: Value = serde_json::from_slice(&durable::read(path)?)?;
        if verdict["schema"] != "tl3.fx_executor_qualification.v1"
            || verdict["status"] != "PASS"
            || verdict["complete"] != true
            || verdict["executor_pathset"]["version"] != policy::PATHSET_VERSION
        {
            return Err("complete PASS at the identical pathset version required".into());
        }
        verify_references(&verdict)?;
        let qualified = verdict["qualified_head"]
            .as_str()
            .ok_or("qualified head absent")?;
        let previous = pathset(repo, qualified)?;
        let current = pathset(repo, head)?;
        let retained: BTreeMap<String, String> =
            serde_json::from_value(verdict["executor_pathset"]["entries"].clone())?;
        if previous != retained
            || verdict["executor_pathset"]["sha256"]
                != durable::digest(&serde_json::to_vec(&previous)?)
        {
            return Err("qualified pathset receipt changed".into());
        }
        let paths: std::collections::BTreeSet<_> = previous.keys().chain(current.keys()).collect();
        changed = paths
            .into_iter()
            .filter(|path| previous.get(*path) != current.get(*path))
            .cloned()
            .collect();
        if !changed.is_empty() {
            return Err("executor source paths changed".into());
        }
        if git(repo, &["rev-parse", "HEAD"])? != head
            || !git(repo, &["status", "--porcelain", "--untracked-files=all"])?.is_empty()
        {
            return Err("carry requires a clean checkout at the requested head".into());
        }
        let generated = d8::generate_checkout(&repo.join("crates/controller"))?;
        let installed = durable::read(&repo.join(policy::PIN))?;
        if generated != installed || verdict["pin"]["sha256"] != durable::digest(&installed) {
            return Err("D8 regeneration or installed pin differs".into());
        }
        let outputs = verdict["pin"]["installed_outputs"]
            .as_array()
            .ok_or("derived inventory absent")?;
        let expected: std::collections::BTreeSet<String> = outputs
            .iter()
            .map(|output| {
                output["relative_path"]
                    .as_str()
                    .map(String::from)
                    .ok_or("relative output missing")
            })
            .collect::<std::result::Result<_, _>>()?;
        let index: d8::current::Index =
            serde_json::from_slice(&durable::read(&repo.join(policy::INDEX))?)?;
        let mut observed: std::collections::BTreeSet<String> = index
            .inputs
            .keys()
            .chain(index.fixtures.keys())
            .map(|name| format!("{}{}", policy::INPUTS, name))
            .collect();
        observed.extend([policy::PIN.into(), policy::INDEX.into()]);
        if expected != observed {
            return Err("derived outputs have changed closed inventory".into());
        }
        for output in outputs {
            let relative = output["relative_path"]
                .as_str()
                .ok_or("relative output missing")?;
            if !policy::derived(relative)
                || identity(&repo.join(relative))?["sha256"] != output["sha256"]
            {
                return Err("derived output differs from qualification".into());
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        reasons.push(error.to_string());
    }
    println!(
        "{}",
        json!({"decision":if reasons.is_empty(){"carry"}else{"rerun"},"head":head,"pathset_version":policy::PATHSET_VERSION,"changed_paths":changed,"reasons":reasons})
    );
    Ok(())
}
