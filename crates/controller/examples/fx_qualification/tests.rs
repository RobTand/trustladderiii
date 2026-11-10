use super::*;

pub(super) fn inputs(root: &Path) -> Inputs {
    Inputs {
        repo: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        head: "e".repeat(40),
        test_image: "sandbox@sha256:test".into(),
        generator_image: PQ.into(),
        image_map: root.join("image-map.json"),
        image_map_sha256: "a".repeat(64),
        out: root.into(),
        readiness_seconds: 120,
    }
}

pub(super) fn canary(root: &Path, kind: &str) -> evidence::CanaryEvidence {
    let receipt = durable::blob(&root.join(format!("{kind}-receipt.json")), b"{}").unwrap();
    let mut checks: Vec<_> = [
        "typed_ending", "clean_event", "verified_cleanup", "scratch_absence",
        "shape_allocation", "job_cgroup_absent", "instance_receipts_complete",
    ].into_iter().collect();
    let ending = match kind {
        "launch_failure" => { checks.push("post_begin_launch_failure"); api::Ending::InfraLost }
        "cancel" => { checks.push("owned_cancel_after_readiness"); api::Ending::Cancelled }
        "success" => { checks.push("real_pytest_base_red_head_green"); api::Ending::Passed }
        "generator" => {
            checks.extend(["bounded_generator_artifact", "target_flux_uri_broker_and_containment_probes", "actual_image_and_read_only_socket_mount"]);
            api::Ending::Passed
        }
        _ => panic!("unknown fixture kind"),
    };
    evidence::CanaryEvidence {
        kind: kind.into(), job: Some(json!({"id":format!("f-{kind}")})),
        status: "PASS".into(), ending: Some(ending), terminal_receipt: Some(receipt.clone()),
        cleanup: Some(api::CleanupResult { stopped: true, evidence: vec![receipt], pending_executions: vec![], accepted_execution: None, reason: "fixture cleanup".into() }),
        executions: vec![], artifacts: vec![],
        checks: checks.into_iter().map(|name| evidence::Check { name: name.into(), pass: true, evidence: vec![] }).collect(),
    }
}

pub(super) fn prepared(inputs: &Inputs) {
    let directory = inputs.out.join("prepared");
    fs::create_dir(&directory).unwrap();
    let helper = b"trusted helper\n";
    fs::write(directory.join("helper.py"), helper).unwrap();
    let mut bundles = BTreeMap::new();
    for (kind, interpreter) in [("success", "/opt/tl3/bin/python"), ("launch_failure", "/opt/tl3/bin/python-canary2-absent")] {
        let manifest = api::RuntimeBundleManifest {
            schema: "tl3.runtime_bundle.v1".into(), owner: "rob".into(), uid: 1000,
            producer_commit: inputs.head.clone(), interpreter: "/usr/bin/python3.14".into(),
            podman: "/usr/bin/podman".into(), container_interpreter: interpreter.into(),
            cleanup_arguments: vec![], collector: "helper.py".into(), cleanup: "helper.py".into(), trusted_root: "lib".into(),
            files: vec![api::BundleFile { path: "helper.py".into(), sha256: durable::digest(helper), bytes: helper.len() as u64, mode: 0o400 }],
        };
        let blob = durable::blob(&directory.join(format!("{kind}-manifest.json")), &serde_json::to_vec(&manifest).unwrap()).unwrap();
        bundles.insert(kind, blob);
    }
    write(&directory.join("result.json"), &json!({"bundles":bundles})).unwrap();
}

#[test]
fn lifecycle_producer_acceptance_round_trips_through_d8() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    prepared(&inputs);
    let canaries: Vec<_> = ["launch_failure", "cancel", "success"].into_iter().map(|kind| canary(directory.path(), kind)).collect();
    let outputs = prepare::lifecycle_inputs(&inputs, &canaries).expect("D8 must accept real producer acceptance files");
    let acceptance: Value = serde_json::from_slice(&outputs[&format!("{}launch_failure-acceptance.json", policy::INPUTS)]).unwrap();
    assert_eq!(serde_json::from_value::<api::Ending>(acceptance["ending"].clone()).unwrap(), api::Ending::InfraLost);
    assert_eq!(outputs[policy::PIN], d8::current::generate(&directory.path().join("private/qualification/current.json")).unwrap());
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.is_empty() { return vec![vec![]]; }
    let mut result = vec![];
    for at in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(at);
        for mut order in permutations(&rest) {
            order.insert(0, first.clone());
            result.push(order);
        }
    }
    result
}

#[test]
fn resume_selects_lifecycle_kinds_in_every_retained_order_after_generator() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    prepared(&inputs);
    let mut original = None;
    for kinds in [&["launch_failure", "cancel", "success"][..], &["launch_failure", "cancel", "success", "generator"][..]] {
        for order in permutations(kinds) {
            let mut retained: Vec<_> = order.iter().map(|kind| canary(directory.path(), kind)).collect();
            let selected = lifecycle_canaries(&retained).unwrap();
            assert_eq!(selected.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), ["launch_failure", "cancel", "success"], "retained order: {order:?}");
            for kind in ["launch_failure", "cancel", "success"] {
                retain_canary(&mut retained, canary(directory.path(), kind));
            }
            let state = State { schema: "tl3.fx_qualification_state.v1".into(), inputs: inputs.clone(), phase: "generator_complete".into(), builds: vec![], canaries: retained };
            save(directory.path(), &state).unwrap();
            let resumed: State = serde_json::from_slice(&durable::read(&directory.path().join("state.json")).unwrap()).unwrap();
            let selected = lifecycle_canaries(&resumed.canaries).unwrap();
            assert_eq!(selected.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), ["launch_failure", "cancel", "success"]);
            let outputs = prepare::lifecycle_inputs(&inputs, &selected).unwrap();
            if let Some(original) = &original { assert_eq!(&outputs, original); }
            else { original = Some(outputs); }
        }
    }
}

#[test]
fn carry_checks_real_git_d8_closed_inventory_and_retained_decisions() {
    let directory = tempfile::tempdir().unwrap();
    let repo = directory.path().join("repo");
    let out = directory.path().join("evidence");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&out).unwrap();
    git(&repo, &["init", "--quiet", "--template=", "-b", "main"]).unwrap();
    fs::write(repo.join("Cargo.toml"), "executor source").unwrap();
    git(&repo, &["add", "--all"]).unwrap();
    git(&repo, &["commit", "--quiet", "-m", "executor"]).unwrap();
    let mut inputs = inputs(&out);
    inputs.repo = repo.clone();
    inputs.head = git(&repo, &["rev-parse", "HEAD"]).unwrap();
    prepared(&inputs);
    let canaries: Vec<_> = ["launch_failure", "cancel", "success", "generator"].into_iter().map(|kind| canary(&out, kind)).collect();
    let outputs = prepare::lifecycle_inputs(&inputs, &lifecycle_canaries(&canaries).unwrap()).unwrap();
    let index: d8::current::Index = serde_json::from_slice(&outputs[policy::INDEX]).unwrap();
    let mut installed = vec![];
    for (relative, bytes) in &outputs {
        let path = repo.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let mode = relative.strip_prefix(policy::INPUTS).and_then(|name| index.fixtures.get(name)).map_or(0o644, |i| i.mode);
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let mut record = identity(&path).unwrap();
        record["relative_path"] = relative.clone().into();
        installed.push(record);
    }
    git(&repo, &["add", "--all"]).unwrap();
    git(&repo, &["commit", "--quiet", "-m", "qualified outputs"]).unwrap();
    let head = git(&repo, &["rev-parse", "HEAD"]).unwrap();
    let entries = evidence::pathset(&repo, &inputs.head).unwrap();
    let verdict = json!({"schema":"tl3.fx_executor_qualification.v1","status":"PASS","complete":true,
        "qualified_head":inputs.head,"canaries":canaries,
        "executor_pathset":{"version":policy::PATHSET_VERSION,"entries":entries,"sha256":durable::digest(&serde_json::to_vec(&entries).unwrap())},
        "pin":{"sha256":durable::digest(&outputs[policy::PIN]),"installed_outputs":installed}});
    let path = out.join("verdict.json");
    let run = |value: &Value, requested: &str| {
        write(&path, value).unwrap();
        command(&["carry".into(), "--repo".into(), repo.clone().into_os_string(), "--verdict".into(), path.clone().into_os_string(), "--head".into(), requested.into()]).unwrap()
    };
    assert_eq!(run(&verdict, &head), 0);
    assert_eq!(run(&verdict, &"f".repeat(40)), 3, "Git failure means rerun");
    let mut unproven = verdict.clone(); unproven["canaries"][0]["checks"] = json!([]);
    assert_eq!(run(&unproven, &head), 3, "PASS labels alone cannot carry");
    let mut inventory = verdict.clone(); inventory["pin"]["installed_outputs"].as_array_mut().unwrap().pop();
    assert_eq!(run(&inventory, &head), 3, "derived inventory must be closed");
    fs::write(repo.join(format!("{}fixtures/unlisted.py", policy::INPUTS)), "unlisted").unwrap();
    git(&repo, &["add", "--all"]).unwrap();
    git(&repo, &["commit", "--quiet", "-m", "unexpected derived file"]).unwrap();
    let changed = git(&repo, &["rev-parse", "HEAD"]).unwrap();
    assert_eq!(run(&verdict, &changed), 3, "D8 rejects unlisted derived files");
}

#[test]
fn lifecycle_inputs_refuse_missing_duplicate_and_foreign_kinds() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    prepared(&inputs);
    for kinds in [["launch_failure", "cancel", "generator"], ["launch_failure", "cancel", "cancel"]] {
        let canaries: Vec<_> = kinds.into_iter().map(|kind| canary(directory.path(), kind)).collect();
        assert!(prepare::lifecycle_inputs(&inputs, &canaries).is_err());
    }
}

#[test]
fn dry_run_exits_zero_only_for_the_completed_preview() {
    assert_eq!(exit_code(&json!({"status":"UNKNOWN","errors":[{"code":"dry_run_no_jobs_submitted"}]}), true), 0);
    assert_eq!(exit_code(&json!({"status":"UNKNOWN","errors":[{"code":"build failed"}]}), true), 2);
    assert_eq!(exit_code(&json!({"status":"UNKNOWN","errors":[{"code":"dry_run_no_jobs_submitted"},{"code":"build failed"}]}), true), 2);
    assert_eq!(exit_code(&json!({"status":"FAIL","errors":[]}), true), 1);
}
