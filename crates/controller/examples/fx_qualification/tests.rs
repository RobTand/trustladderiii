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
    for kinds in [&["launch_failure", "cancel", "success"][..], &["launch_failure", "cancel", "success", "generator"][..]] {
        for order in permutations(kinds) {
            let retained: Vec<_> = order.iter().map(|kind| canary(directory.path(), kind)).collect();
            let selected = lifecycle_canaries(&retained).unwrap();
            assert_eq!(selected.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), ["launch_failure", "cancel", "success"], "retained order: {order:?}");
        }
    }
}
