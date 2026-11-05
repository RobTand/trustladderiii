use std::io::Write;
use std::process::{Command, Stdio};

/// #840: real class-D issues retain their supplied tests at both downstream inputs.
pub fn run() {
    let mut child = Command::new(super::worker_package().join(".venv/bin/python"))
        .args(["-c", include_str!("ingest_verbatim/worker.py")])
        .env("TL3_BINARY", env!("CARGO_BIN_EXE_tl3"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("ingest_verbatim/golden.json"))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!("test ingest_verbatim_real_issue_tests_reach_planner_and_maker ... ok");
}

/// Stored stdout from an older release still follows ingest-done into Auditing.
pub fn stored_reply(settings: &tl3_config::Store) {
    use super::*;
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(&root.path().join("fakes")).unwrap(),
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    let mut supervisor = Supervisor::new(
        &config,
        root.path(),
        &mut crate::startup_ownership::ownership(),
    )
    .unwrap();
    let run = delivery();
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    fs::write(
        root_for(&config, &run.key).join("delivery.json"),
        json!(run).to_string(),
    )
    .unwrap();
    fs::write(attempt.join("exit.json"), "{\"code\":0}").unwrap();
    let case: Value = serde_json::from_str::<Value>(include_str!("ingest_verbatim/golden.json"))
        .unwrap()[0]
        .clone();
    let mut reply = case["reply"].clone();
    reply["files"] = json!([]);
    reply["evidence_contract"] = json!([]);
    let stdout = format!(
        "{}\n",
        json!({"event":"reply","role":"ingest","reply":reply})
    );
    fs::write(attempt.join("stdout.jsonl"), &stdout).unwrap();
    for capabilities in [None, Some(json!([]))] {
        let mut inputs = json!({"issue":case["issue"]});
        if let Some(capabilities) = capabilities {
            inputs["tool_capabilities"] = capabilities;
        }
        fs::write(
            attempt.join("request.json"),
            json!({"role":"ingest","inputs":inputs}).to_string(),
        )
        .unwrap();
        let completion = terminal(&mut supervisor, &run);
        assert!(
            !failed(&completion),
            "stored pre-#840 ingest reply became RunFailed"
        );
        let [Observation::WorkerReply { reply: checked, .. }] = completion.observations.as_slice()
        else {
            panic!("stored ingest reply was not accepted");
        };
        assert_eq!(
            checked["brief"], reply["brief"],
            "stored draft and forecast stay intact"
        );
        assert_eq!(
            fs::read_to_string(attempt.join("stdout.jsonl")).unwrap(),
            stdout
        );
        if controller.world().unwrap().nodes[0].phase == Phase::Ingesting {
            for observation in completion.observations {
                controller.observe("stored-ingest", observation).unwrap();
            }
        }
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Auditing);
        assert_eq!(controller.world().unwrap().nodes[0].run_failures, 0);
        let audit = adapters::workers::check_ingest_reply(
            "audit",
            &json!({"issue":case["issue"],"ingest":reply}),
            &json!({"verdict":"agree","objections":[]}),
        )
        .unwrap();
        assert_eq!(audit.reply["verdict"], "disagree");
    }
    println!("test stored_pre_840_ingest_revalidates_then_audit_objects ... ok");
}
