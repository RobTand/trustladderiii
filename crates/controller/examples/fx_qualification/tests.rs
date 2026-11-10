use super::*;

#[test]
fn preparation_identity_proofs_round_trip_through_live_reader() {
    let root = tempfile::tempdir().unwrap();
    for (name, raw) in [
        (
            "absent-interpreter-preflight.stdout",
            &b"{\"absent\":true}\n"[..],
        ),
        ("rootless.stdout", &b"true\n"[..]),
    ] {
        let path = root.path().join(name);
        durable::atomic(&path, raw).unwrap();
        let produced = identity(&path).unwrap();
        assert!(produced["mode"].is_u64());
        let (proof, observed) = live::read_identity_proof(&produced, root.path())
            .expect("preparation identity must be consumable as a checked BlobRef");
        assert_eq!(proof.path, path);
        assert_eq!(proof.sha256, durable::digest(raw));
        assert_eq!(proof.bytes, raw.len() as u64);
        assert_eq!(observed, raw);
    }
}

#[test]
fn preparation_identity_reader_keeps_digest_size_and_path_checks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = root.path().join("proof.json");
    durable::atomic(&path, b"{}").unwrap();
    let produced = identity(&path).unwrap();
    live::read_identity_proof(&produced, root.path()).unwrap();
    for field in ["sha256", "bytes"] {
        let mut changed = produced.clone();
        changed[field] = if field == "sha256" {
            "0".repeat(64).into()
        } else {
            3.into()
        };
        assert!(
            live::read_identity_proof(&changed, root.path())
                .unwrap_err()
                .to_string()
                .contains("output digest mismatch")
        );
    }
    let external = outside.path().join("proof.json");
    durable::atomic(&external, b"{}").unwrap();
    assert!(
        live::read_identity_proof(&identity(&external).unwrap(), root.path())
            .unwrap_err()
            .to_string()
            .contains("output outside owned directory")
    );
    let mut changed_mode = produced.clone();
    changed_mode["mode"] = 0.into();
    assert_eq!(
        evidence::verify_references(&changed_mode).unwrap_err().to_string(),
        "referenced artifact mode changed"
    );
    assert!(serde_json::from_value::<api::BlobRef>(produced).is_err());
}

#[test]
fn coordinator_inputs_and_state_use_the_production_file_codec() {
    let root = tempfile::tempdir().unwrap();
    let expected = inputs(root.path());
    let input_path = root.path().join("inputs.json");
    write(&input_path, &expected).unwrap();
    assert_eq!(read_json::<Inputs>(&input_path).unwrap(), expected);
    let state = State {
        schema: "tl3.fx_qualification_state.v1".into(),
        inputs: expected,
        phase: "lifecycle".into(),
        builds: vec![],
        canaries: ["launch_failure", "cancel", "success", "generator"]
            .into_iter()
            .map(|kind| canary(root.path(), kind))
            .collect(),
    };
    save(root.path(), &state).unwrap();
    let resumed: State = read_json(&root.path().join("state.json")).unwrap();
    assert_eq!(
        serde_json::to_value(resumed).unwrap(),
        serde_json::to_value(state).unwrap()
    );
}

#[test]
fn typed_driver_artifacts_round_trip_through_production_writers_and_reader() {
    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(root: &Path, name: &str, value: &T) {
        let path = root.join(name);
        write(&path, value).unwrap();
        let parsed: T = read_json(&path).unwrap();
        assert_eq!(
            serde_json::to_value(parsed).unwrap(),
            serde_json::to_value(value).unwrap(),
            "artifact {name}"
        );
        durable::atomic(&path, &serde_json::to_vec_pretty(value).unwrap()).unwrap();
        let parsed: T = read_json(&path).unwrap();
        assert_eq!(
            serde_json::to_value(parsed).unwrap(),
            serde_json::to_value(value).unwrap(),
            "atomic artifact {name}"
        );
    }
    let root = tempfile::tempdir().unwrap();
    let flux = tl3_config::Flux {
        activation: None,
        instance_file: root.path().join("INSTANCE"),
        executable: root.path().join("flux"),
        backend_cli: root.path().join("tl3-run-backend"),
        environment: tl3_controller::production::flux_job_environment(),
        container_mode: tl3_config::ContainerMode::InJob,
        runtime_bundles: vec![],
        staging_root: root.path().join("staging"),
        output_root: root.path().join("output"),
        container_image: format!("fixture.invalid/sandbox@sha256:{}", "a".repeat(64)),
    };
    let options = tl3_controller::production::flux_options(&flux);
    for name in ["context.json", "held-options.json", "generator-options.json"] {
        round_trip(root.path(), name, &options);
    }
    round_trip(root.path(), "job-environment.json", &options.environment);
    round_trip(
        root.path(),
        "mode-prerequisites.json",
        &api::ModePrerequisites {
            no_cgroups_supported: true,
            init_available: true,
            user_scope_supported: false,
        },
    );
    let delivery = super::super::fixture(
        &root.path().join("served"),
        super::super::FixtureKind::Success,
    )
    .unwrap();
    round_trip(root.path(), "delivery.json", &delivery);
    let profile = tl3_controller::adapters::workers::execution::Profile {
        pass_flux_uri: generator::PASS_FLUX_URI,
        activation: None,
        schema: "tl3.worker_execution.v1".into(),
        attempt: root.path().join("attempt"),
        checkout: root.path().join("checkout"),
        delivery,
        options: root.path().join("generator-options.json"),
        job_environment: root.path().join("job-environment.json"),
        staging_root: root.path().join("staging"),
        runtime_bundles: vec![],
        container_mode: tl3_config::ContainerMode::InJob,
        waits_root: root.path().join("waits"),
    };
    round_trip(root.path(), "execution.json", &profile);
    for kind in ["launch_failure", "cancel", "success", "generator"] {
        round_trip(root.path(), &format!("{kind}-result.json"), &canary(root.path(), kind));
    }
}

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
        "typed_ending",
        "clean_event",
        "verified_cleanup",
        "scratch_absence",
        "shape_allocation",
        "job_cgroup_absent",
        "instance_receipts_complete",
    ]
    .into_iter()
    .collect();
    let ending = match kind {
        "launch_failure" => {
            checks.push("post_begin_launch_failure");
            api::Ending::InfraLost
        }
        "cancel" => {
            checks.push("owned_cancel_after_readiness");
            api::Ending::Cancelled
        }
        "success" => {
            checks.push("real_pytest_base_red_head_green");
            api::Ending::Passed
        }
        "generator" => {
            checks.extend([
                "bounded_generator_artifact",
                "target_flux_absence_and_containment_probes",
                "actual_image_and_socket_mount_absence",
            ]);
            api::Ending::Passed
        }
        _ => panic!("unknown fixture kind"),
    };
    evidence::CanaryEvidence {
        kind: kind.into(),
        job: Some(json!({"id":format!("f-{kind}")})),
        status: "PASS".into(),
        ending: Some(ending),
        terminal_receipt: Some(receipt.clone()),
        cleanup: Some(api::CleanupResult {
            stopped: true,
            evidence: vec![receipt],
            pending_executions: vec![],
            accepted_execution: None,
            reason: "fixture cleanup".into(),
        }),
        executions: vec![],
        artifacts: vec![],
        checks: checks
            .into_iter()
            .map(|name| evidence::Check {
                name: name.into(),
                pass: true,
                evidence: vec![],
            })
            .collect(),
    }
}

pub(super) fn prepared(inputs: &Inputs) {
    let directory = inputs.out.join("prepared");
    fs::create_dir(&directory).unwrap();
    let helper = b"trusted helper\n";
    fs::write(directory.join("helper.py"), helper).unwrap();
    let mut bundles = BTreeMap::new();
    for (kind, interpreter) in [
        ("success", "/opt/tl3/bin/python"),
        ("launch_failure", "/opt/tl3/bin/python-canary2-absent"),
    ] {
        let manifest = api::RuntimeBundleManifest {
            schema: "tl3.runtime_bundle.v1".into(),
            owner: "rob".into(),
            uid: 1000,
            producer_commit: inputs.head.clone(),
            interpreter: "/usr/bin/python3.14".into(),
            podman: "/usr/bin/podman".into(),
            container_interpreter: interpreter.into(),
            cleanup_arguments: vec![],
            collector: "helper.py".into(),
            cleanup: "helper.py".into(),
            trusted_root: "lib".into(),
            files: vec![api::BundleFile {
                path: "helper.py".into(),
                sha256: durable::digest(helper),
                bytes: helper.len() as u64,
                mode: 0o400,
            }],
        };
        let blob = durable::blob(
            &directory.join(format!("{kind}-manifest.json")),
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        bundles.insert(kind, blob);
    }
    write(&directory.join("result.json"), &json!({"bundles":bundles})).unwrap();
}

#[test]
fn lifecycle_producer_acceptance_round_trips_through_d8() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    prepared(&inputs);
    let canaries: Vec<_> = ["launch_failure", "cancel", "success"]
        .into_iter()
        .map(|kind| canary(directory.path(), kind))
        .collect();
    let outputs = prepare::lifecycle_inputs(&inputs, &canaries)
        .expect("D8 must accept real producer acceptance files");
    let acceptance: Value = serde_json::from_slice(
        &outputs[&format!("{}launch_failure-acceptance.json", policy::INPUTS)],
    )
    .unwrap();
    assert_eq!(
        serde_json::from_value::<api::Ending>(acceptance["ending"].clone()).unwrap(),
        api::Ending::InfraLost
    );
    assert_eq!(
        outputs[policy::PIN],
        d8::current::generate(&directory.path().join("private/qualification/current.json"))
            .unwrap()
    );
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.is_empty() {
        return vec![vec![]];
    }
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
    for kinds in [
        &["launch_failure", "cancel", "success"][..],
        &["launch_failure", "cancel", "success", "generator"][..],
    ] {
        for order in permutations(kinds) {
            let mut retained: Vec<_> = order
                .iter()
                .map(|kind| canary(directory.path(), kind))
                .collect();
            let selected = lifecycle_canaries(&retained).unwrap();
            assert_eq!(
                selected.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(),
                ["launch_failure", "cancel", "success"],
                "retained order: {order:?}"
            );
            for kind in ["launch_failure", "cancel", "success"] {
                retain_canary(&mut retained, canary(directory.path(), kind));
            }
            let state = State {
                schema: "tl3.fx_qualification_state.v1".into(),
                inputs: inputs.clone(),
                phase: "generator_complete".into(),
                builds: vec![],
                canaries: retained,
            };
            save(directory.path(), &state).unwrap();
            let resumed: State = serde_json::from_slice(
                &durable::read(&directory.path().join("state.json")).unwrap(),
            )
            .unwrap();
            let selected = lifecycle_canaries(&resumed.canaries).unwrap();
            assert_eq!(
                selected.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(),
                ["launch_failure", "cancel", "success"]
            );
            let outputs = prepare::lifecycle_inputs(&inputs, &selected).unwrap();
            if let Some(original) = &original {
                assert_eq!(&outputs, original);
            } else {
                original = Some(outputs);
            }
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
    let canaries: Vec<_> = ["launch_failure", "cancel", "success", "generator"]
        .into_iter()
        .map(|kind| canary(&out, kind))
        .collect();
    let outputs =
        prepare::lifecycle_inputs(&inputs, &lifecycle_canaries(&canaries).unwrap()).unwrap();
    let index: d8::current::Index = serde_json::from_slice(&outputs[policy::INDEX]).unwrap();
    let mut installed = vec![];
    for (relative, bytes) in &outputs {
        let path = repo.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let mode = relative
            .strip_prefix(policy::INPUTS)
            .and_then(|name| index.fixtures.get(name))
            .map_or(0o644, |i| i.mode);
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
        durable::atomic(&path, &serde_json::to_vec(value).unwrap()).unwrap();
        command(&[
            "carry".into(),
            "--repo".into(),
            repo.clone().into_os_string(),
            "--verdict".into(),
            path.clone().into_os_string(),
            "--head".into(),
            requested.into(),
        ])
        .unwrap()
    };
    assert_eq!(run(&verdict, &head), 0);
    assert_eq!(run(&verdict, &"f".repeat(40)), 3, "Git failure means rerun");
    let mut unproven = verdict.clone();
    unproven["canaries"][0]["checks"] = json!([]);
    assert_eq!(run(&unproven, &head), 3, "PASS labels alone cannot carry");
    let mut inventory = verdict.clone();
    inventory["pin"]["installed_outputs"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_eq!(
        run(&inventory, &head),
        3,
        "derived inventory must be closed"
    );
    fs::write(
        repo.join(format!("{}fixtures/unlisted.py", policy::INPUTS)),
        "unlisted",
    )
    .unwrap();
    git(&repo, &["add", "--all"]).unwrap();
    git(
        &repo,
        &["commit", "--quiet", "-m", "unexpected derived file"],
    )
    .unwrap();
    let changed = git(&repo, &["rev-parse", "HEAD"]).unwrap();
    assert_eq!(
        run(&verdict, &changed),
        3,
        "D8 rejects unlisted derived files"
    );
}

#[test]
fn lifecycle_inputs_refuse_missing_duplicate_and_foreign_kinds() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    prepared(&inputs);
    for (kinds, expected) in [
        (
            ["launch_failure", "cancel", "generator"],
            "missing lifecycle canary kind",
        ),
        (
            ["launch_failure", "cancel", "cancel"],
            "duplicate lifecycle canary kind",
        ),
    ] {
        let canaries: Vec<_> = kinds
            .into_iter()
            .map(|kind| canary(directory.path(), kind))
            .collect();
        assert_eq!(
            prepare::lifecycle_inputs(&inputs, &canaries)
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}

#[test]
fn dry_run_exits_zero_only_for_the_completed_preview() {
    assert_eq!(
        exit_code(
            &json!({"status":"UNKNOWN","errors":[{"code":"dry_run_no_jobs_submitted"}]}),
            true
        ),
        0
    );
    assert_eq!(
        exit_code(
            &json!({"status":"UNKNOWN","errors":[{"code":"build failed"}]}),
            true
        ),
        2
    );
    assert_eq!(
        exit_code(
            &json!({"status":"UNKNOWN","errors":[{"code":"dry_run_no_jobs_submitted"},{"code":"build failed"}]}),
            true
        ),
        2
    );
    assert_eq!(exit_code(&json!({"status":"FAIL","errors":[]}), true), 1);
}

#[test]
fn pq_production_canary_does_not_require_live_flux() {
    assert!(
        !generator::PASS_FLUX_URI,
        "production PQ profile disables Flux transport"
    );
}

#[test]
fn pq_tool_preflight_requires_flux_only_when_forwarded() {
    use std::process::Command;
    let directory = tempfile::tempdir().unwrap();
    for (git, flux, forwarded, pass) in [
        (true, false, false, true),
        (true, false, true, false),
        (false, true, false, false),
        (false, true, true, false),
        (true, true, false, true),
        (true, true, true, true),
    ] {
        for (name, present) in [("git", git), ("flux", flux)] {
            let path = directory.path().join(name);
            if present {
                fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            } else if path.exists() {
                fs::remove_file(&path).unwrap();
            }
        }
        let output = Command::new("/usr/bin/python3")
            .env_clear()
            .env("PATH", directory.path())
            .args(["-c", prepare::pq_tool_preflight(forwarded)])
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            pass,
            "git={git} flux={flux} forwarded={forwarded}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !pass {
            assert!(String::from_utf8_lossy(&output.stderr).contains("AssertionError"));
        }
    }
}

#[test]
fn qualification_plan_uses_production_pq_and_ascii_job_ids() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    let planned = plan(&inputs);
    assert_eq!(planned["serial"], true);
    assert_eq!(planned["flux_job_count"], 7);
    let jobs = planned["jobs"].as_array().unwrap();
    for job in jobs {
        assert_eq!(
            job["resources"],
            json!([{"type":"slot","count":1,"label":"task","with":[{"type":"core","count":1},{"type":"memory","count":2}]}])
        );
        assert_eq!(
            job["attributes"]["system"]["environment"],
            json!({"FLUX_F58_FORCE_ASCII":"1"})
        );
        assert_eq!(job["environment_policy"]["start_empty"], true);
        assert_eq!(job["environment_policy"]["pass_flux_uri"], false);
        assert_eq!(job["rlimit_flags"], json!(["--rlimit=-*"]));
        assert_eq!(job["submission"], "NOT_SUBMITTED");
        assert_eq!(job["gpus"], 0);
        assert_eq!(job["parallelism"], 1);
        assert_eq!(job["native_threads"], 1);
        assert!(job["attributes"]["system"]["duration"].as_u64().unwrap() <= 600);
    }
}
