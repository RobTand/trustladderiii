#[path = "../../store/tests/support/mod.rs"]
mod maintenance_store_support;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tl3_controller::run_backend::{self as api, durable::digest, *};

fn cli(request: &Value) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Every suite Python that may import from a source checkout runs with `-B`. The flag is
/// on the interpreter argv, so it survives the `env_clear()` of product spawn paths that
/// drops PYTHONDONTWRITEBYTECODE; the checkout stays byte-clean.
const PYTHON_NO_BYTECODE: &str = "-B";

/// Rob 11-03: a performance-budget overrun is a recorded warning, never a failure
/// or a rerun. Only a contractual deadline (a product timeout) fails a test.
/// The line follows the repo `{"budget_warning": ...}` convention (tools/bazel/rust_runner.py).
fn check_budget(test: &str, start: Instant, budget: Duration) {
    let measured = start.elapsed();
    if measured > budget {
        println!(
            "{}",
            json!({"budget_warning": {"test": test, "measured": measured.as_secs_f64(),
                "limit": budget.as_secs_f64(), "metric": "wall"}})
        );
    }
}

/// The run-backend transport idle timeout: a CLI child silent this long is hung by contract.
fn transport_idle_deadline() -> Duration {
    Duration::from_secs(cleanup_limits().idle_timeout_seconds)
}

/// The native command deadline; a stop_watch that is not faster cannot be told from expiry.
fn native_command_deadline() -> Duration {
    Duration::from_secs(cleanup_limits().native_timeout_seconds)
}

/// Failing bound for a stop_watch measured from `stopped`, which is taken after the
/// native call (and its deadline) began. Deadline expiry lands near 30 s - δ after
/// `stopped`; 10 s of margin keeps a dropped or ignored stop on the failing side.
fn stop_watch_deadline() -> Duration {
    native_command_deadline() - Duration::from_secs(10)
}

/// Test-only latency knob: unset is 0; anything else must be finite, non-negative seconds.
fn latency_knob(name: &str) -> f64 {
    std::env::var(name).map_or(0.0, |value| {
        value
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .unwrap_or_else(|| panic!("{name} must be non-negative seconds, got {value:?}"))
    })
}

/// Wait for a fixture milestone: a warning past `budget`, a failure only at the idle timeout.
fn wait_until(test: &str, budget: Duration, mut reached: impl FnMut() -> bool) {
    let start = Instant::now();
    while !reached() {
        assert!(
            start.elapsed() < transport_idle_deadline(),
            "{test}: not reached within the transport idle timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    check_budget(test, start, budget);
}

/// Receive one frame: a warning past `budget`, a failure only at the idle timeout.
fn recv_within<T>(test: &str, receive: &std::sync::mpsc::Receiver<T>, budget: Duration) -> T {
    let start = Instant::now();
    let value = receive
        .recv_timeout(transport_idle_deadline())
        .unwrap_or_else(|error| {
            panic!("{test}: no frame within the transport idle timeout: {error}")
        });
    check_budget(test, start, budget);
    value
}

#[test]
fn cli_schema_is_generated_from_rust_and_describes_all_operations() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"));
    let output = command.arg("schema").output().unwrap();
    let schema: Value = serde_json::from_slice(&output.stdout).unwrap();
    let committed: Value =
        serde_json::from_str(include_str!("../src/run_backend/schemas/run-backend.json")).unwrap();
    assert_eq!(
        schema, committed,
        "regenerate the committed schema using the Rust CLI"
    );
    let launch: Value = serde_json::from_str(&api::schema::launch_context().unwrap()).unwrap();
    let committed_launch: Value = serde_json::from_str(include_str!(
        "../src/run_backend/schemas/run-backend-launch-context.json"
    ))
    .unwrap();
    assert_eq!(
        launch, committed_launch,
        "regenerate the committed launch schema using the Rust CLI"
    );
    let text = schema.to_string();
    for operation in [
        "submit",
        "attach",
        "cancel",
        "probe",
        "cleanup_runtime",
        "cleanup_limits",
        "cleanup_progress",
    ] {
        assert!(text.contains(operation), "missing operation {operation}");
    }
    assert!(text.contains("LaunchContext"));
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generated_action_refusal_is_typed_prompt_and_releases_scratch() {
    generated_action_case("refusal");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generated_action_post_begin_launch_failure_is_a_candidate_until_external_absence() {
    let f = generated_action_case("absent-interpreter");
    launch_failure_acceptance_matrix(&f);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generated_action_post_begin_launch_failure_with_cancel_retires_debt() {
    let f = generated_action_case("absent-interpreter");
    let state_path = f
        .options
        .journal_root
        .join(&f.intent.correlation)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["cancel_requested"] = json!(true);
    fs::write(&state_path, state.to_string()).unwrap();
    let mut events = completed(0);
    events.insert(
        1,
        json!({"timestamp":1.5,"name":"exception","context":{"type":"cancel","severity":0,"note":"fixture cancellation before finish"}}),
    );
    f.events(&events);
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(fs::read_dir(&debt).unwrap().next().is_some());
    let frames = f.attach();
    let terminal = &frames.last().unwrap()["result"]["observation"]["terminal"];
    assert_eq!(terminal["ending"], "infra-lost", "{frames:?}");
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.intent.output_dir.join("receipt.json")).unwrap())
            .unwrap();
    assert!(
        receipt["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["path"].as_str().unwrap().contains("unit-absence-") })
    );
    assert!(fs::read_dir(&debt).unwrap().next().is_none());
    assert!(!f.request.scratch_root.join("fABC").exists());
    assert!(
        f.intent
            .output_dir
            .join("scratch-reclamation.manifest.json")
            .exists()
    );
    let repeated = f.attach();
    assert_eq!(
        repeated.last().unwrap()["result"]["observation"]["terminal"],
        *terminal,
        "{repeated:?}"
    );
    assert!(fs::read_dir(&debt).unwrap().next().is_none());
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join("accepted-launch-failure-with-cancel");
        fs::create_dir_all(&destination).unwrap();
        fs::copy(
            f.intent.output_dir.join("receipt.json"),
            destination.join("receipt.json"),
        )
        .unwrap();
        for entry in fs::read_dir(&f.intent.output_dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        fs::copy(&state_path, destination.join("state.json")).unwrap();
        fs::write(
            destination.join("attach.json"),
            serde_json::to_vec(&frames).unwrap(),
        )
        .unwrap();
        fs::write(
            destination.join("repeat-attach.json"),
            serde_json::to_vec(&repeated).unwrap(),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generated_action_consumes_native_grant_before_binding_and_payload() {
    for case in [
        "success",
        "affinity-mismatch",
        "resource-mismatch",
        "changed-allocation",
        "binding-mismatch",
        "begin-response-lost",
    ] {
        generated_action_case(case);
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn action_session_rechecks_generation_owner_and_live_allocation() {
    generated_action_case("session");
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn generated_action_sigterm_unwinds_workspace() {
    let f = generated_action_case("cancel-term");
    finish_cancel_fixture(&f, false);
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn generated_action_sigterm_unconfirmed_cleanup_exits_without_later_checks() {
    let f = generated_action_case("cancel-term-unconfirmed");
    let state: Value = serde_json::from_slice(
        &fs::read(
            f.options
                .journal_root
                .join(&f.intent.correlation)
                .join("state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(state["container_launches"].as_object().unwrap().len(), 1);
    assert_eq!(state["runtime_ownerships"].as_array().unwrap().len(), 1);
    assert!(state["cleanup_evidence"].as_object().unwrap().is_empty());
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    f.events(&[
        json!({"name":"start","context":{}}),
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"finish","context":{"status":143}}),
        json!({"name":"clean"}),
    ]);
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{frames:?}"
    );
    assert!(f.request.scratch_root.join("fABC").exists());
    assert!(
        !f.intent
            .output_dir
            .join("scratch-reclamation.manifest.json")
            .exists()
    );
    assert!(!f.intent.output_dir.join("receipt.json").exists());
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(fs::read_dir(debt).unwrap().next().is_some());
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join("cancel-term-unconfirmed");
        fs::write(
            destination.join("attach-unknown.json"),
            serde_json::to_vec(&frames).unwrap(),
        )
        .unwrap();
        fs::write(destination.join("unconfirmed-assertions.json"), json!({"launches":1,"no_later_checks":true,"scratch_retained":true,"debt_retained":true,"terminal_absent":true}).to_string()).unwrap();
    }
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn generated_action_sigkill_reclaims_after_cancel_and_timeout() {
    for timeout in [false, true] {
        let f = generated_action_case(if timeout {
            "timeout-kill"
        } else {
            "cancel-kill"
        });
        finish_cancel_fixture(&f, timeout);
    }
}

fn finish_cancel_fixture(f: &Fixture, timeout: bool) {
    if !timeout {
        let frames = f
            .call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_eq!(
            frames.last().unwrap()["result"]["acknowledgement"]["acknowledged"],
            true
        );
    }
    f.events(&[
        json!({"name":"start","context":{}}),
        json!({"name":"exception","context":{"type":if timeout {"timeout"} else {"cancel"},"severity":0}}),
        json!({"name":"finish","context":{"status":9}}),
        json!({"name":"clean"}),
    ]);
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        if timeout { "infra-lost" } else { "cancelled" },
        "{frames:?}"
    );
    assert!(!f.request.scratch_root.join("fABC").exists());
    let repeated = f.attach();
    assert_eq!(
        repeated.last().unwrap()["result"]["observation"]["terminal"],
        frames.last().unwrap()["result"]["observation"]["terminal"]
    );
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(fs::read_dir(debt).unwrap().next().is_none());
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join(if timeout {
            "timeout-terminal"
        } else {
            "cancel-terminal"
        });
        fs::create_dir_all(&destination).unwrap();
        for entry in fs::read_dir(&f.intent.output_dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        fs::write(
            destination.join("attach.json"),
            serde_json::to_vec(&frames).unwrap(),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn scratch_reclamation_serializes_begin_until_terminal_and_rejects_late_begin() {
    let f = generated_action_case("success-retained");
    let root = f.request.scratch_root.join("fABC");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("leftover"), b"admitted timeout scratch").unwrap();
    f.events(&[
        json!({"name":"start","context":{}}),
        json!({"name":"exception","context":{"type":"timeout","severity":0}}),
        json!({"name":"finish","context":{"status":9}}),
        json!({"name":"clean"}),
    ]);
    let state: Value = serde_json::from_slice(
        &fs::read(
            f.options
                .journal_root
                .join(&f.intent.correlation)
                .join("state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut launcher: RuntimeOwnership =
        serde_json::from_value(state["runtime_ownerships"][0].clone()).unwrap();
    launcher.execution = "waiting-launch".into();
    launcher.container = None;
    let command = json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher});
    fs::write(
        f.intent.output_dir.join("scratch-pause"),
        b"pause before tree operation",
    )
    .unwrap();
    std::thread::scope(|scope| {
        let attach = scope.spawn(|| f.attach());
        wait_until(
            "helper reaches reclamation pause",
            Duration::from_secs(10),
            || f.intent.output_dir.join("scratch-entered").exists(),
        );
        assert!(root.join("leftover").exists());
        assert!(!f.intent.output_dir.join("receipt.json").exists());
        let gate = fs::File::open(f.options.journal_root.join("runtime-lock")).unwrap();
        assert!(gate.try_lock().is_err(), "reclamation must own launch gate");
        let (send, receive) = std::sync::mpsc::channel();
        let waiting_command = command.clone();
        let fixture = &f;
        scope.spawn(move || {
            send.send(fixture.call(waiting_command)).unwrap();
        });
        assert!(
            receive
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "begin escaped reclamation gate"
        );
        fs::write(
            f.intent.output_dir.join("scratch-release"),
            b"release own fixture helper",
        )
        .unwrap();
        let terminal = attach.join().unwrap();
        assert_eq!(
            terminal.last().unwrap()["result"]["observation"]["terminal"]["ending"],
            "infra-lost",
            "{terminal:?}"
        );
        assert!(!root.exists());
        let waiting = recv_within(
            "waiting begin returns after reclamation",
            &receive,
            Duration::from_secs(10),
        );
        assert_eq!(waiting.last().unwrap()["kind"], "error", "{waiting:?}");
        assert!(
            waiting.last().unwrap()["message"]
                .as_str()
                .unwrap()
                .contains("after cancellation or terminal")
        );
        let late = f.call(command.clone());
        assert_eq!(late.last().unwrap()["kind"], "error", "{late:?}");
        assert!(
            late.last().unwrap()["message"]
                .as_str()
                .unwrap()
                .contains("after cancellation or terminal")
        );
        let final_state: Value = serde_json::from_slice(
            &fs::read(
                f.options
                    .journal_root
                    .join(&f.intent.correlation)
                    .join("state.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(final_state["container_launches"]["waiting-launch"].is_null());
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            let destination = Path::new(&destination).join("scratch-launch-race");
            fs::create_dir_all(&destination).unwrap();
            fs::write(destination.join("assertions.json"), json!({"helper_paused_before_mutation":true,"gate_held":true,"waiting_begin_blocked":true,"terminal":terminal,"waiting_begin":waiting,"late_begin":late,"final_state":final_state}).to_string()).unwrap();
        }
    });
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn scratch_preflight_requires_helper_and_retained_host_authority() {
    for case in ["no-helper", "no-ownership", "host-absent"] {
        let f = generated_action_case(case);
        let report: Value =
            serde_json::from_slice(&fs::read(f.intent.output_dir.join("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["outcome"], "checks_completed", "{case}: {report}");
        let stdout = fs::read_to_string(f.intent.output_dir.join("host.stdout")).unwrap();
        let failures = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("TL3_EXECUTION_FAILURE "))
            .map(|raw| serde_json::from_str::<Value>(raw).unwrap())
            .collect::<Vec<_>>();
        assert!(!failures.is_empty());
        assert!(
            failures
                .iter()
                .all(|failure| failure["outcome"] == "setup_failure"
                    && failure["termination_reason"] == "prelaunch_refused")
        );
        let refusals = fs::read_dir(&f.intent.output_dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".runtime.jsonl")
            })
            .flat_map(|entry| {
                fs::read_to_string(entry.path())
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .map(|raw| serde_json::from_str::<Value>(&raw).unwrap())
            .filter(|event| event["event"] == "prelaunch_refusal")
            .collect::<Vec<_>>();
        assert!(!refusals.is_empty());
        assert!(
            refusals
                .iter()
                .all(|refusal| refusal["command_released"] == false
                    && refusal["launcher_started"] == false)
        );
        let state: Value = serde_json::from_slice(
            &fs::read(
                f.options
                    .journal_root
                    .join(&f.intent.correlation)
                    .join("state.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(state["container_launches"].as_object().unwrap().is_empty());
        assert!(state["runtime_ownerships"].as_array().unwrap().is_empty());
        f.events(&completed(0));
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{case}: {frames:?}"
        );
        assert!(!f.intent.output_dir.join("receipt.json").exists());
        assert!(
            !f.intent
                .output_dir
                .join("scratch-reclamation.manifest.json")
                .exists()
        );
        let debt = f
            .options
            .journal_root
            .join("cleanup-debt")
            .join(&f.request.runtime.manifest_digest);
        assert!(fs::read_dir(debt).unwrap().next().is_some());
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            fs::write(
                Path::new(&destination)
                    .join(case)
                    .join("attach-unknown.json"),
                serde_json::to_vec(&frames).unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn scratch_reclamation_rejects_manifest_completion_and_attribution_mutations() {
    let f = generated_action_case("success-retained");
    let root = f.request.scratch_root.join("fABC");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("leftover"), b"owned rejection fixture").unwrap();
    f.events(&[
        json!({"name":"start","context":{}}),
        json!({"name":"exception","context":{"type":"timeout","severity":0}}),
        json!({"name":"finish","context":{"status":9}}),
        json!({"name":"clean"}),
    ]);
    for (case, mutation) in [
        ("schema", json!({"manifest_replace":{"schema":"foreign"}})),
        ("path", json!({"manifest_replace":{"path":"/foreign"}})),
        ("uid", json!({"manifest_replace":{"uid":0}})),
        ("run", json!({"manifest_replace":{"run":null}})),
        ("owner", json!({"manifest_replace":{"owner":null}})),
        (
            "digest",
            json!({"manifest_replace":{"request_digest":"foreign"}}),
        ),
        (
            "attribution",
            json!({"manifest_replace":{"attribution":"action cleanup"}}),
        ),
        (
            "entries-bound",
            json!({"manifest_replace":{"entries":vec![json!({});4097]}}),
        ),
        ("entries-type", json!({"manifest_replace":{"entries":null}})),
        (
            "bytes-bound",
            json!({"manifest_replace":{"total_bytes":2_u64*1024*1024*1024+1}}),
        ),
        (
            "bytes-type",
            json!({"manifest_replace":{"total_bytes":null}}),
        ),
        ("completion-missing", json!({"completion":"missing"})),
        ("completion-unequal", json!({"completion":"unequal"})),
        (
            "pre-removal-generation-missing",
            json!({"replace":{"generation_pre_removal":null}}),
        ),
        (
            "manifest-path",
            json!({"replace":{"manifest":"/foreign/manifest.json"}}),
        ),
        ("unsolicited-attribution", json!({})),
    ] {
        if case == "unsolicited-attribution" {
            f.events(&completed(0));
        }
        let mut mutation = mutation;
        mutation["reclamation_result"] = json!(true);
        fs::write(
            f.intent.output_dir.join("scratch-control.json"),
            mutation.to_string(),
        )
        .unwrap();
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{case}: {frames:?}"
        );
        assert!(
            !root.exists(),
            "first genuine authorized reclamation must remove only own root"
        );
        assert!(!f.intent.output_dir.join("receipt.json").exists());
        let debt = f
            .options
            .journal_root
            .join("cleanup-debt")
            .join(&f.request.runtime.manifest_digest)
            .join(format!("{}.scratch", f.intent.correlation));
        assert!(debt.exists());
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            let destination = Path::new(&destination)
                .join("reclamation-result-negatives")
                .join(case);
            fs::create_dir_all(&destination).unwrap();
            for name in [
                "scratch-control.json",
                "scratch-reclamation.manifest.json",
                "scratch-reclamation.complete.json",
            ] {
                if f.intent.output_dir.join(name).exists() {
                    fs::copy(f.intent.output_dir.join(name), destination.join(name)).unwrap();
                }
            }
            fs::write(
                destination.join("attach-unknown.json"),
                serde_json::to_vec(&frames).unwrap(),
            )
            .unwrap();
            fs::write(destination.join("assertions.json"), json!({"terminal_absent":true,"scratch_debt_retained":true,"no_additional_tree_removal":true,"reclamation_result_rejected":true}).to_string()).unwrap();
        }
    }
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn terminal_scratch_guards_keep_ordinary_leftovers_and_reject_bad_external_evidence() {
    let f = generated_action_case("success-retained");
    let root = f.request.scratch_root.join("fABC");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        root.join("leftover"),
        b"ordinary action cleanup bug fixture",
    )
    .unwrap();
    f.events(&completed(0));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{frames:?}"
    );
    assert!(root.join("leftover").exists());
    assert!(
        !f.intent
            .output_dir
            .join("scratch-reclamation.manifest.json")
            .exists()
    );
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest)
        .join(format!("{}.scratch", f.intent.correlation));
    assert!(debt.exists());
    fs::remove_dir_all(&root).unwrap(); // Remove only this admitted fixture's injected tree.
    for (name, replacement) in [
        ("host", json!({"host":"foreign"})),
        (
            "owner",
            json!({"owner":{"owner":"foreign","correlation":f.intent.correlation}}),
        ),
        (
            "correlation",
            json!({"owner":{"owner":f.intent.owner,"correlation":"foreign"}}),
        ),
        ("request", json!({"request_digest":"d".repeat(64)})),
        ("nonce", json!({"nonce":"d".repeat(64)})),
        ("path", json!({"path":"/tmp/foreign"})),
        ("uid", json!({"uid":0})),
        (
            "stale",
            json!({"started_monotonic_ns":0,"finished_monotonic_ns":0}),
        ),
        (
            "duration",
            json!({"started_monotonic_ns":1,"finished_monotonic_ns":20_000_000_000_u64}),
        ),
        (
            "run",
            json!({"run":{"backend":"flux","instance":"foreign","uri":"local:///fixture","job":"fABC"}}),
        ),
        ("attribution", json!({"attribution":"action cleanup"})),
    ] {
        fs::write(
            f.intent.output_dir.join("scratch-control.json"),
            json!({"replace":replacement}).to_string(),
        )
        .unwrap();
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{name}: {frames:?}"
        );
        assert!(debt.exists() && !f.intent.output_dir.join("receipt.json").exists());
    }
    fs::write(
        f.intent.output_dir.join("scratch-control.json"),
        json!({"replace":{
            "generation_before":{"exit_code":0,"stdout":"after-reboot","stderr":""},
            "generation_after":{"exit_code":0,"stdout":"after-reboot","stderr":""}
        }})
        .to_string(),
    )
    .unwrap();
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "passed",
        "{frames:?}"
    );
    assert!(!debt.exists());
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cleanup_helper_import_from_a_checkout_writes_no_bytecode() {
    // The product cleanup spawn clears the environment, so PYTHONDONTWRITEBYTECODE never
    // reaches the helper. A private checkout copy shows exactly what the helper writes.
    let checkout = tempfile::tempdir().unwrap();
    let package = checkout.path().join("harness/src/tl3_harness");
    fs::create_dir_all(&package).unwrap();
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/src/tl3_harness");
    for entry in fs::read_dir(&harness).unwrap().flatten() {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "py")
        {
            fs::copy(entry.path(), package.join(entry.file_name())).unwrap();
        }
    }
    let imported = checkout.path().join("imported");
    let init = package.join("__init__.py");
    let mut text = fs::read_to_string(&init).unwrap();
    text.push_str(&format!(
        "\nopen({:?}, 'w').close()\n",
        imported.to_str().unwrap()
    ));
    fs::write(&init, text).unwrap();
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    let scratch = tempfile::tempdir().unwrap();
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    f.request.scratch_root = scratch.path().to_owned();
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    fs::write(
        f.root.path().join("action-fixture.json"),
        json!({"source": checkout.path()}).to_string(),
    )
    .unwrap();
    assert_eq!(f.submit()["kind"], "accepted");
    f.retain_fixture_allocation();
    f.events(&completed(256));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "infra-lost",
        "{frames:?}"
    );
    assert!(
        imported.exists(),
        "the helper must import tl3_harness from the checkout"
    );
    fn bytecode(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "__pycache__")
                || path.extension().is_some_and(|extension| extension == "pyc")
            {
                found.push(path.clone());
            }
            if entry.file_type().unwrap().is_dir() {
                bytecode(&path, found);
            }
        }
    }
    let mut written = Vec::new();
    bytecode(checkout.path(), &mut written);
    assert!(
        written.is_empty(),
        "bytecode written into the checkout: {written:?}"
    );
}

fn generated_action_case(mode: &str) -> Fixture {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    if mode == "no-helper" {
        f.request.runtime.runtime = ExecutionRuntime::Native {
            interpreter: "/usr/bin/python3".into(),
        };
        f.request.runtime.cleanup = None;
    }
    let scratch = tempfile::tempdir().unwrap();
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    f.request.scratch_root = scratch.path().to_owned();
    f.request.reservation.cpus = 1.try_into().unwrap();
    f.request.reservation.parallelism = 1.try_into().unwrap();
    f.request.suite = Suite::DaytimeChecks {
        base: f.request.tree.head.clone(),
        head: f.request.tree.head.clone(),
        reviewed: None,
        plan_digest: "a".repeat(64),
    };
    f.request.no_progress_seconds = Some(10.try_into().unwrap());
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    f.submit();
    let mut environment = f.request.environment.clone();
    for (name, value) in [
        ("TL3_EXECUTION_BACKEND", "flux".to_owned()),
        (
            "TL3_RUN_BACKEND_CLI",
            env!("CARGO_BIN_EXE_tl3-run-backend").to_owned(),
        ),
        (
            "TL3_RUN_BACKEND_CONFIG",
            f.config.to_string_lossy().into_owned(),
        ),
        ("TL3_RUN_OWNER", f.intent.owner.clone()),
        ("TL3_RUN_CORRELATION", f.intent.correlation.clone()),
        (
            "TL3_RUN_DIAGNOSTIC_DIR",
            f.intent.output_dir.to_string_lossy().into_owned(),
        ),
        ("FLUX_JOB_ID", "fABC".to_owned()),
        ("TL3_RUN_BACKEND_SESSION", "1".to_owned()),
        ("OUT", f.intent.output_dir.to_string_lossy().into_owned()),
    ] {
        environment.insert(name.into(), value);
    }
    fs::write(
        f.root.path().join("action-fixture.json"),
        serde_json::to_vec(&json!({
            "environment": environment, "output_dir": f.intent.output_dir,
            "scratch": scratch.path().join("fABC"), "mode": mode,
            "injected_latency_seconds": latency_knob("FX_TEST_ACTION_LATENCY_SECONDS"),
            "source": Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap(),
        }))
        .unwrap(),
    )
    .unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let action_source = std::env::var_os("FX_ACTION_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| source.to_owned());
    let output = Command::new("/home/rob/venvs/pb-cpu/bin/python")
        .arg(PYTHON_NO_BYTECODE)
        .arg(source.join("harness/tests/fx_action_e2e.py"))
        .arg(f.root.path())
        .arg(action_source)
        .arg(mode)
        .output()
        .unwrap();
    eprintln!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join(mode);
        fs::create_dir_all(&destination).unwrap();
        for entry in fs::read_dir(&f.intent.output_dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        for name in [
            "calls",
            "all.json",
            "runtime-calls.jsonl",
            "action-fixture.json",
        ] {
            let file = f.root.path().join(name);
            if file.is_file() {
                fs::copy(file, destination.join(name)).unwrap();
            }
        }
        fs::copy(
            f.root
                .path()
                .join("journal")
                .join(&f.intent.correlation)
                .join("state.json"),
            destination.join("state.json"),
        )
        .unwrap();
        fs::write(destination.join("driver.stdout"), &output.stdout).unwrap();
        fs::write(destination.join("driver.stderr"), &output.stderr).unwrap();
    }
    assert!(
        output.status.success(),
        "generated action refusal regression"
    );
    if mode == "success" {
        f.events(&completed(0));
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
            "passed",
            "{frames:?}"
        );
        assert!(
            !fs::read_dir(&f.intent.output_dir)
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("unit-absence-"))
        );
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            fs::copy(
                f.intent.output_dir.join("receipt.json"),
                Path::new(&destination).join(mode).join("receipt.json"),
            )
            .unwrap();
        }
    }
    f.scratch = Some(scratch);
    f
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn refused_before_tests_never_invokes_external_unit_absence() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    f.submit();
    f.retain_fixture_allocation();
    fs::write(f.intent.output_dir.join("report.json"), json!({
        "schema":"tl3.runner_report.v1", "run":f.ownership().run,
        "request_digest":f.intent.request_digest, "tree":f.request.tree,
        "runner_digest":f.request.runner.executable_digest, "runtime_digest":f.request.runtime.manifest_digest,
        "suite":f.request.suite, "complete":false, "cleanup_complete":true,
        "tests_started":0, "outcome":"refused_before_tests", "cases":[], "payload":null,
        "runtime_binding":null, "runtime_bindings":[], "cleanup":null, "stdout":null, "stderr":null, "worker_payload":null
    }).to_string()).unwrap();
    f.events(&completed(0));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "refused",
        "{frames:?}"
    );
    assert!(
        !fs::read_dir(&f.intent.output_dir)
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("unit-absence-"))
    );
}

fn launch_failure_acceptance_matrix(f: &Fixture) {
    let out = &f.intent.output_dir;
    let state_path = f
        .options
        .journal_root
        .join(&f.intent.correlation)
        .join("state.json");
    let original_state = fs::read(&state_path).unwrap();
    let original_report = fs::read(out.join("report.json")).unwrap();
    let report: Value = serde_json::from_slice(&original_report).unwrap();
    let proofs = report["launch_failures"].as_array().unwrap();
    assert_eq!(
        proofs.len(),
        2,
        "both actual generated launches must be retained"
    );
    let process_path = PathBuf::from(proofs[0]["process_record"]["path"].as_str().unwrap());
    let process_raw = fs::read(&process_path).unwrap();
    let cleanup_path = PathBuf::from(proofs[0]["cleanup"]["path"].as_str().unwrap());
    let cleanup_raw = fs::read(&cleanup_path).unwrap();
    let unknown = |case: &str| {
        assert!(
            f.request.scratch_root.join("fABC").exists(),
            "candidate scratch disappeared before {case}"
        );
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{case}: {frames:?}"
        );
        assert!(
            !out.join("receipt.json").exists(),
            "{case} committed a terminal"
        );
        let debt = f
            .options
            .journal_root
            .join("cleanup-debt")
            .join(&f.request.runtime.manifest_digest);
        assert!(
            fs::read_dir(debt).unwrap().next().is_some(),
            "{case} lost cleanup debt"
        );
        assert!(
            f.request.scratch_root.join("fABC").exists(),
            "UNKNOWN {case} reclaimed scratch"
        );
        assert!(!out.join("scratch-reclamation.manifest.json").exists());
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            let destination = Path::new(&destination).join("acceptance-matrix").join(case);
            fs::create_dir_all(&destination).unwrap();
            for entry in fs::read_dir(out).unwrap().flatten() {
                if entry.file_type().unwrap().is_file() {
                    fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
                }
            }
            fs::write(
                destination.join("state.json"),
                fs::read(&state_path).unwrap(),
            )
            .unwrap();
            fs::write(
                destination.join("attach.json"),
                serde_json::to_vec(&frames).unwrap(),
            )
            .unwrap();
        }
    };
    // No native Clean means the new helper is unreachable.
    f.events(&[]);
    unknown("missing-native-clean");
    assert!(!fs::read_dir(out).unwrap().flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with("unit-absence-")
    }));
    f.events(&completed(0));
    for (case, replacement) in [
        (
            "control-missing",
            json!({"control":{"path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice","observation":{"kind":"absent","errno":2}}}),
        ),
        (
            "control-unreadable",
            json!({"control":{"path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice","observation":{"kind":"unknown","errno":13}}}),
        ),
        (
            "control-file",
            json!({"control":{"path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice","observation":{"kind":"not_directory"}}}),
        ),
        ("missing-control", json!({"control":null})),
        (
            "wrong-control-path",
            json!({"control":{"path":"/sys/fs/cgroup/foreign.slice","observation":{"kind":"directory"}}}),
        ),
        (
            "control-directory-job-present",
            json!({"observation":{"kind":"present"}}),
        ),
        (
            "unit-unreadable",
            json!({"observation":{"kind":"unknown","errno":13}}),
        ),
        (
            "unit-present-populated0",
            json!({"observation":{"kind":"present","populated":0}}),
        ),
        ("wrong-host", json!({"host":"foreign-host"})),
        (
            "sibling-unit",
            json!({"path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABD:sys.service"}),
        ),
        (
            "foreign-unit",
            json!({"path":"/sys/fs/cgroup/foreign.slice/shell-fABC:sys.service"}),
        ),
        (
            "shared-slice",
            json!({"path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice"}),
        ),
        (
            "wrong-correlation",
            json!({"owner":{"owner":"fixture-owner","correlation":"foreign"}}),
        ),
        (
            "wrong-run",
            json!({"run":{"backend":"flux","instance":"fixture-generation","uri":"local:///fixture","job":"fABD"}}),
        ),
        (
            "wrong-instance",
            json!({"run":{"backend":"flux","instance":"foreign-generation","uri":"local:///fixture","job":"fABC"}}),
        ),
        ("wrong-request", json!({"request_digest":"f".repeat(64)})),
        ("stale-nonce", json!({"nonce":"0".repeat(64)})),
        (
            "stale-time",
            json!({"started_monotonic_ns":1,"finished_monotonic_ns":2}),
        ),
        ("missing-before", json!({"generation_before":null})),
        ("missing-after", json!({"generation_after":null})),
        ("missing-unit", json!({"observation":null})),
        ("missing-nonce", json!({"nonce":null})),
    ] {
        fs::write(
            out.join("unit-control.json"),
            json!({"replace":replacement}).to_string(),
        )
        .unwrap();
        unknown(case);
    }
    fs::write(
        out.join("unit-control.json"),
        json!({"exit_code":1}).to_string(),
    )
    .unwrap();
    unknown("helper-nonzero");
    fs::remove_file(out.join("unit-control.json")).unwrap();
    for case in [
        "name-present",
        "labels-present",
        "launcher-present",
        "group-present",
        "missing-pending",
        "missing-post-exit-time",
        "pre-exit-absence",
    ] {
        let mut cleanup: Value = serde_json::from_slice(&cleanup_raw).unwrap();
        match case {
            "name-present" => cleanup["pending_launch"]["name_exists_exit"] = json!(0),
            "labels-present" => {
                cleanup["pending_launch"]["remaining_ids"] = json!(["a".repeat(64)])
            }
            "launcher-present" => {
                cleanup["launcher"] = json!({"kind":"present","observed_start_ticks":1})
            }
            "group-present" => cleanup["process_group_probe_errno"] = Value::Null,
            "missing-pending" => cleanup["pending_launch"] = Value::Null,
            "missing-post-exit-time" => {
                cleanup["pending_launch"]
                    .as_object_mut()
                    .unwrap()
                    .remove("started_monotonic_ns");
            }
            "pre-exit-absence" => cleanup["pending_launch"]["started_monotonic_ns"] = json!(1),
            _ => unreachable!(),
        }
        let reference =
            api::durable::blob(&cleanup_path, &serde_json::to_vec(&cleanup).unwrap()).unwrap();
        let mut state: Value = serde_json::from_slice(&original_state).unwrap();
        state["cleanup_evidence"][proofs[0]["execution"].as_str().unwrap()] =
            serde_json::to_value(&reference).unwrap();
        let mut changed_report = report.clone();
        changed_report["launch_failures"][0]["cleanup"] = serde_json::to_value(&reference).unwrap();
        let mut rows: Vec<Value> = String::from_utf8(process_raw.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        rows.iter_mut()
            .find(|row| row["event"] == "no_container_observed")
            .unwrap()["cleanup"] = serde_json::to_value(reference).unwrap();
        let bytes = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        let reference = api::durable::blob(&process_path, bytes.as_bytes()).unwrap();
        changed_report["launch_failures"][0]["process_record"] =
            serde_json::to_value(reference).unwrap();
        fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        fs::write(out.join("report.json"), changed_report.to_string()).unwrap();
        unknown(case);
    }
    fs::write(&cleanup_path, &cleanup_raw).unwrap();
    fs::write(&state_path, &original_state).unwrap();
    for case in [
        "missing-exit",
        "missing-exit-status",
        "exit0",
        "forced-exit",
        "missing-release",
        "missing-intent",
        "missing-absence",
        "observed-with-matching-stderr",
        "stderr-only",
    ] {
        let mut rows: Vec<Value> = String::from_utf8(process_raw.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        match case {
            "missing-exit" => rows.retain(|row| row["event"] != "launcher_exit"),
            "missing-exit-status" => {
                rows.iter_mut()
                    .find(|row| row["event"] == "launcher_exit")
                    .unwrap()["exit_code"] = Value::Null
            }
            "exit0" => {
                rows.iter_mut()
                    .find(|row| row["event"] == "launcher_exit")
                    .unwrap()["exit_code"] = json!(0)
            }
            "forced-exit" => {
                rows.iter_mut()
                    .find(|row| row["event"] == "launcher_exit")
                    .unwrap()["natural_exit"] = json!(false)
            }
            "missing-release" => {
                rows.iter_mut()
                    .find(|row| row["event"] == "launcher_exit")
                    .unwrap()["command_released"] = json!(false)
            }
            "missing-intent" => rows.retain(|row| row["event"] != "launch_intent"),
            "missing-absence" => rows.retain(|row| row["event"] != "no_container_observed"),
            "observed-with-matching-stderr" => {
                rows.push(json!({"event":"container","Cpus_allowed_list":"0","cgroup":"0::/"}))
            }
            "stderr-only" => {
                rows = vec![
                    json!({"event":"stderr","stderr_tail":"catatonit: failed to exec pid1: No such file or directory"}),
                ]
            }
            _ => unreachable!(),
        }
        let bytes = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        let reference = api::durable::blob(&process_path, bytes.as_bytes()).unwrap();
        let mut changed = report.clone();
        changed["launch_failures"][0]["process_record"] = serde_json::to_value(reference).unwrap();
        fs::write(out.join("report.json"), changed.to_string()).unwrap();
        unknown(case);
    }
    fs::write(&process_path, &process_raw).unwrap();
    fs::write(out.join("report.json"), &original_report).unwrap();
    let mut state: Value = serde_json::from_slice(&original_state).unwrap();
    let mut observed = state["container_launches"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()["launcher"]
        .clone();
    observed["container"] =
        json!({"mode":"in_job","id":"c".repeat(64),"name":"observed","labels":{},"scope":null});
    state["runtime_ownerships"] = json!([observed]);
    fs::write(&state_path, state.to_string()).unwrap();
    unknown("mixed-bound-and-failed");
    fs::write(&state_path, &original_state).unwrap();
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "infra-lost",
        "{frames:?}"
    );
    let receipt: Value =
        serde_json::from_slice(&fs::read(out.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["report"].as_object().unwrap().len(), 3);
    assert!(!f.request.scratch_root.join("fABC").exists());
    assert!(out.join("scratch-reclamation.manifest.json").exists());
    assert!(
        receipt["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"].as_str().unwrap().contains("unit-absence-"))
    );
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join("accepted-launch-failure");
        fs::create_dir_all(&destination).unwrap();
        for entry in fs::read_dir(out).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        fs::write(
            destination.join("attach.json"),
            serde_json::to_vec(&frames).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn malformed_requests_have_a_versioned_bounded_error_frame() {
    assert!(!api::token("."));
    assert!(!api::token(".."));
    let frames = cli(&json!({"op":"attach", "backend":"prismabuild", "key":"pb-old-key"}));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["schema"], "tl3.run_backend.frame.v1");
    assert_eq!(frames[0]["kind"], "error");
    assert_eq!(frames[0]["code"], "invalid_request");
    assert_eq!(frames[0]["sequence"], 0);
}

struct Fixture {
    root: tempfile::TempDir,
    request: Request,
    intent: SubmissionIntent,
    options: Options,
    config: PathBuf,
    membership: Vec<u8>,
    bundles: Vec<PathBuf>,
    scratch: Option<tempfile::TempDir>,
}

fn immutable(path: &Path, bytes: &[u8], executable: bool) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }),
    )
    .unwrap();
}

fn fixture_instance(root: &Path) -> PathBuf {
    let locator = root.join("INSTANCE");
    fs::write(&locator, b"local:///fixture\n").unwrap();
    immutable(
        &root.join("flux-uri.sh"),
        b"#!/bin/sh\nexec cat \"$(dirname \"$0\")/INSTANCE\"\n",
        true,
    );
    locator
}

impl Fixture {
    fn new() -> Self {
        let shared = Path::new("/mnt/shared/flux-out/tl3");
        fs::create_dir_all(shared).unwrap();
        let root = tempfile::Builder::new()
            .prefix("fx1-fixture-")
            .tempdir_in(shared)
            .unwrap();
        let p = root.path();
        let correlation = format!("{}-out", p.file_name().unwrap().to_str().unwrap());
        let tree = p.join("tree");
        fs::create_dir(&tree).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("/usr/bin/git")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .arg("-C")
                .arg(&tree)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git fixture setup failed");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--quiet"]);
        fs::write(tree.join("subject.txt"), b"immutable subject\n").unwrap();
        git(&["add", "subject.txt"]);
        git(&[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ]);
        let head = git(&["rev-parse", "HEAD"]);
        let tree_id = git(&["rev-parse", "HEAD^{tree}"]);
        immutable(&tree.join("subject.txt"), b"immutable subject\n", false);
        let input = serde_json::to_vec(&InputManifest {
            schema: "tl3.input_manifest.v1".into(),
            head: head.clone(),
            tree: tree_id.clone(),
            files: vec![InputFile {
                path: "subject.txt".into(),
                sha256: digest(b"immutable subject\n"),
            }],
        })
        .unwrap();
        immutable(&p.join("input.json"), &input, false);
        immutable(&p.join("runner"), b"#!/bin/sh\nexit 0\n", true);
        immutable(&p.join("runner.json"), b"{\"runner\":1}", false);
        immutable(&p.join("runtime.json"), b"{\"runtime\":1}", false);
        immutable(
            &p.join("preflight.json"),
            &serde_json::to_vec(&RuntimePreflight {
                schema: "tl3.runtime_preflight.v1".into(),
                architecture: "x86_64".into(),
                image_identity: None,
                image_present: false,
                identity_verified: true,
                image: None,
                mode: None,
                no_cgroups_supported: false,
                init_available: false,
                user_scope_supported: false,
            })
            .unwrap(),
            false,
        );
        let preflight = fs::read(p.join("preflight.json")).unwrap();
        let membership = serde_json::to_vec(&Membership {
            schema: "tl3.membership.v1".into(),
            cases: vec![CaseIdentity {
                target: "unit".into(),
                name: "case".into(),
            }],
        })
        .unwrap();
        let native_cleanup = p.join("native-cleanup");
        immutable(
            &native_cleanup,
            include_str!("python/fx_native_scratch_helper.py").as_bytes(),
            true,
        );
        let request = Request {
            pass_flux_uri: false,
            repo: "fixture/repo".into(),
            tree: TreeIdentity {
                head,
                tree: tree_id,
                input_digest: digest(&input),
                root: tree,
                manifest: p.join("input.json"),
            },
            suite: Suite::SelectedCases {
                membership_digest: digest(&membership),
            },
            reservation: Reservation {
                cpus: 2.try_into().unwrap(),
                gpus: 0,
                memory_bytes: (1024 * 1024 * 1024).try_into().unwrap(),
                memory_guarantee: MemoryGuarantee::DeclaredOnly,
                parallelism: 2.try_into().unwrap(),
                native_threads: 1.try_into().unwrap(),
            },
            runner: RunnerIdentity {
                executable: p.join("runner"),
                executable_digest: digest(b"#!/bin/sh\nexit 0\n"),
                arguments: vec!["--fixture".into()],
                manifest: p.join("runner.json"),
                manifest_digest: digest(b"{\"runner\":1}"),
            },
            environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            runtime: RuntimeIdentity {
                manifest: p.join("runtime.json"),
                manifest_digest: digest(b"{\"runtime\":1}"),
                runtime: ExecutionRuntime::Native {
                    interpreter: "/usr/bin/python3".into(),
                },
                architecture: "x86_64".into(),
                expected_image: None,
                preflight: BlobRef {
                    sha256: digest(&preflight),
                    bytes: preflight.len() as u64,
                    path: p.join("preflight.json"),
                },
                host_dependency: Some(HostDependency {
                    host: "dl380g10".into(),
                    reason: "fixture of the provisioned amd64 image dependency".into(),
                }),
                image: None,
                cleanup: Some(CleanupProgram {
                    executable: native_cleanup.clone(),
                    executable_digest: digest(&fs::read(&native_cleanup).unwrap()),
                    arguments: vec![],
                }),
            },
            scratch_root: "/var/tmp/flux-vanilla".into(),
            no_progress_seconds: None,
            walltime_seconds: 60.try_into().unwrap(),
        };
        let intent = SubmissionIntent {
            backend: BackendTag::Flux,
            instance: "fixture-generation".into(),
            uri: "local:///fixture".into(),
            owner: "fixture-owner".into(),
            correlation: correlation.clone(),
            request_digest: digest(&serde_json::to_vec(&request).unwrap()),
            output_dir: shared.join(&correlation),
        };
        let config = p.join("options.json");
        let native = p.join("native-flux");
        fs::write(&native, r#"#!/bin/sh
set -eu
fixture=$1
shift
printf '%s\n' "$*" >> "$fixture/calls"
mode=$(cat "$fixture/mode")
# Test-only contention model: delay every native call, never change its result. The
# watched quiet call is exempt: its start anchors the stop_watch deadline in the tests.
case "$mode:$1:${2:-}" in
quiet-jobs:jobs:*|quiet-info:job:info|quiet:job:eventlog|quiet-collect:job:status) ;;
*) if [ -e "$fixture/native-latency" ]; then /bin/sleep "$(cat "$fixture/native-latency")"; fi;;
esac
case "$1" in
submit)
    printf '%s\n' "$@" > "$fixture/args"
    /usr/bin/python3 "$fixture/jobspec.py" "$fixture" "$@";;
jobs) if [ "$mode" = query-error ]; then exit 1; fi; if [ "$mode" = quiet-jobs ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi
    if [ "$mode" = held-query ] && mkdir "$fixture/query-once" 2>/dev/null; then
        touch "$fixture/query-started"
        while ! test -e "$fixture/query-release"; do /bin/sleep 0.01; done
    fi
    if [ "$mode" = terminal-query ] || [ "$mode" = terminal-query-error ]; then
        mv "$fixture/query-terminal-state.json" "$(cat "$fixture/query-state-path")"
        if [ "$mode" = terminal-query-error ]; then exit 1; fi
    fi
    cat "$fixture/jobs.json";;
cancel) printf 'acknowledged\n';;
resource)
    case "$3" in --states=all) cat "$fixture/all.json";; --states=free) cat "$fixture/free.json";; *) exit 9;; esac;;
job)
    case "$2" in
    submit) cp "$3" "$fixture/jobspec.json"; if [ "$mode" = unknown ]; then exit 1; fi; cat "$fixture/submit-id";;
    info)
        if [ "$mode" = quiet-info ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi
        if [ "$3" = --original ]; then if [ -e "$fixture/jobspec-$4.json" ]; then cat "$fixture/jobspec-$4.json"; else cat "$fixture/jobspec.json"; fi; else cat "$fixture/all.json"; fi;;
    eventlog)
        if [ "$mode" = quiet ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi
        if [ "$mode" = interrupted ]; then head -n 1 "$fixture/events"; exit 1; fi
        cat "$fixture/events";;
    status) if [ "$mode" = quiet-collect ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi; printf '{"result":1}\n';;
    *) exit 9;; esac;;
*) exit 9;; esac
"#).unwrap();
        fs::write(p.join("jobspec.py"), r#"import json,sys,pathlib,os,resource
root=pathlib.Path(sys.argv[1]);args=sys.argv[2:]
# Read only the owned fake-native process initial environment, never the test caller.
initial=dict(item.decode().split('=',1) for item in pathlib.Path(f'/proc/{os.getppid()}/environ').read_bytes().split(b'\0') if item)
(root/'native-access-environment.json').write_text(json.dumps(initial))
def option(prefix): return next(item[len(prefix):] for item in args if item.startswith(prefix))
environment=dict(os.environ)
for item in args:
    if item == '--env=-*': environment.clear()
    elif item.startswith('--env=') and '=' in item[6:]:
        key,value=item[6:].split('=',1);environment[key]=value
user=dict(item[15:].split('=',1) for item in args if item.startswith('--setattr=user.'))
shell_options={'output':{'stdout':{'path':option('--output=')},'stderr':{'path':option('--error=')}}}
# Model the Flux CLI: it propagates the submitter's soft rlimits unless --rlimit=-* is given.
if '--rlimit=-*' not in args: shell_options['rlimit']={'nofile':resource.getrlimit(resource.RLIMIT_NOFILE)[0],'core':resource.getrlimit(resource.RLIMIT_CORE)[0]}
runner=next(i for i,item in enumerate(args) if not item.startswith('-') and item.startswith('/mnt/shared/'))
leaves=[{'type':key,'count':int(value)} for key,value in (item.split('=') for item in option('--resources-shape=').removeprefix('slot/[').removesuffix(']').split(';'))]
spec={'tasks':[{'command':args[runner:]}], 'resources':[{'type':'slot','count':1,'label':'task','with':leaves}], 'attributes':{'user':user,'system':{'duration':option('-t') if any(item.startswith('-t=') for item in args) else args[args.index('-t')+1], 'constraints':option('--requires='), 'cwd':option('--cwd='),'environment':environment,'job':{'name':option('--job-name=')}, 'shell':{'options':shell_options}}}}
text=json.dumps(spec);(root/'jobspec.json').write_text(text);print(text)
"#).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o555)).unwrap();
        fs::write(p.join("mode"), "normal").unwrap();
        let native_latency = latency_knob("FX_TEST_NATIVE_LATENCY_SECONDS");
        if native_latency > 0.0 {
            fs::write(p.join("native-latency"), native_latency.to_string()).unwrap();
        }
        fs::write(p.join("submit-id"), "fABC\n").unwrap();
        let all = json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":"0-3"}}],"nodelist":["fixture-host"]}});
        fs::write(p.join("all.json"), all.to_string()).unwrap();
        fs::write(p.join("free.json"), all.to_string()).unwrap();
        fs::write(
            p.join("jobs.json"),
            json!({"jobs":[{"name":correlation,"jobid":"fABC","id":42}]}).to_string(),
        )
        .unwrap();
        let options = Options {
            executable: native,
            arguments: vec![p.to_string_lossy().into_owned()],
            environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            instance_file: fixture_instance(p),
            journal_root: p.join("journal"),
            bootstrap: HostBootstrap {
                cli: env!("CARGO_BIN_EXE_tl3-run-backend").into(),
                config: config.clone(),
            },
        };
        fs::write(&config, serde_json::to_vec(&options).unwrap()).unwrap();
        Self {
            root,
            request,
            intent,
            options,
            config,
            membership,
            bundles: vec![],
            scratch: None,
        }
    }

    fn call(&self, command: Value) -> Vec<Value> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
            .args(["--config", self.config.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{command}").unwrap();
        let output = child.wait_with_output().unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn submit(&self) -> Value {
        let frames = self.call(json!({"op":"submit","request":self.request,"intent":self.intent}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        frames.last().unwrap()["result"]["submission"].clone()
    }
    fn ownership(&self) -> RuntimeOwnership {
        RuntimeOwnership {
            execution: "fixture-execution".into(),
            run: BackendId {
                backend: BackendTag::Flux,
                instance: self.intent.instance.clone(),
                uri: self.intent.uri.clone(),
                job: "fABC".into(),
            },
            owner: Owner {
                owner: self.intent.owner.clone(),
                correlation: self.intent.correlation.clone(),
            },
            request_digest: self.intent.request_digest.clone(),
            host: "dl380g10".into(),
            rank: 0,
            launcher: ProcessIdentity {
                pid: 1234,
                start_ticks: 5678,
            },
            process_group: 1234,
            container: None,
        }
    }
    fn write_fixture_allocation(&self) -> String {
        let count = self.request.reservation.cpus.get();
        let cpus = (0..count)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let nodes = std::iter::once(json!({"metadata":{"type":"node","rank":0,"id":0,"name":"dl380g10"}}))
            .chain((0..count).map(|n| json!({"metadata":{"type":"core","rank":0,"id":n,"properties":{"os-pus":n.to_string()}}}))).collect::<Vec<_>>();
        fs::write(self.root.path().join("all.json"), json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":cpus}}],"nodelist":["dl380g10"]},"scheduling":{"graph":{"nodes":nodes}}}).to_string()).unwrap();
        cpus
    }
    fn observe_fixture_allocation(&self, cpus: &str) -> Vec<Value> {
        self.call(json!({"op":"observe_allocation","owner":self.intent.owner,"correlation":self.intent.correlation,
            "observed":{"run":self.ownership().run,"request_digest":self.intent.request_digest,"hostname":"dl380g10","allowed_cpus":cpus,
            "cgroup":"0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service"}}))
    }
    fn retain_fixture_allocation(&self) {
        // The runner validates its allocation once, before any launch; a later
        // report must not observe again after cancellation.
        if self.state().allocation_grant.is_some() {
            return;
        }
        let cpus = self.write_fixture_allocation();
        let observed = self.observe_fixture_allocation(&cpus);
        assert_eq!(observed.last().unwrap()["kind"], "result", "{observed:?}");
        let frames = self.call(json!({"op":"validate_allocation","owner":self.intent.owner,"correlation":self.intent.correlation,"facts":observed.last().unwrap()["result"]["facts"]}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    }

    fn report(&self, outcome: ReportOutcome, cleanup_complete: bool) {
        self.retain_fixture_allocation();
        let out = &self.intent.output_dir;
        fs::write(out.join("membership.json"), &self.membership).unwrap();
        let owned = self.ownership();
        let frames = self.call(json!({"op":"retain_runtime","owner":self.intent.owner,"correlation":self.intent.correlation,"ownership":owned}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        let cleanup_bytes = serde_json::to_vec(&CleanupEvidence {
            schema: "tl3.cleanup_evidence.v1".into(),
            pending_launch: None,
            ownership: owned,
            launcher: ProcessStop::Reaped { status: 0 },
            process_group_probe_errno: Some(3),
            container: None,
        })
        .unwrap();
        fs::write(out.join("cleanup.txt"), &cleanup_bytes).unwrap();
        if cleanup_complete {
            let frames = self.call(json!({"op":"retain_cleanup","owner":self.intent.owner,"correlation":self.intent.correlation,
                "evidence":BlobRef { sha256: digest(&cleanup_bytes), bytes: cleanup_bytes.len() as u64, path: out.join("cleanup.txt") }}));
            assert_eq!(
                frames.last().unwrap()["result"]["cleanup"]["stopped"],
                true,
                "{frames:?}"
            );
        }
        let report = RunnerReport {
            schema: "tl3.runner_report.v1".into(),
            run: BackendId {
                backend: BackendTag::Flux,
                instance: self.intent.instance.clone(),
                uri: self.intent.uri.clone(),
                job: "fABC".into(),
            },
            request_digest: self.intent.request_digest.clone(),
            tree: self.request.tree.clone(),
            runner_digest: self.request.runner.executable_digest.clone(),
            runtime_digest: self.request.runtime.manifest_digest.clone(),
            suite: self.request.suite.clone(),
            complete: true,
            cleanup_complete,
            tests_started: 1,
            outcome,
            cases: vec![CaseFact {
                target: "unit".into(),
                name: "case".into(),
                outcome: if outcome == ReportOutcome::TestsFailed {
                    CaseOutcome::Failed
                } else {
                    CaseOutcome::Passed
                },
                duration_ms: 1,
                disposition: None,
            }],
            payload: None,
            runtime_binding: None,
            runtime_bindings: vec![],
            launch_failures: vec![],
            cleanup: Some(BlobRef {
                sha256: digest(&cleanup_bytes),
                bytes: cleanup_bytes.len() as u64,
                path: out.join("cleanup.txt"),
            }),
            stdout: None,
            stderr: None,
            worker_payload: None,
        };
        fs::write(
            out.join("report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
    }
    fn events(&self, events: &[Value]) {
        let text = events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>();
        fs::write(self.root.path().join("events"), text).unwrap();
    }
    fn attach(&self) -> Vec<Value> {
        self.call(
            json!({"op":"attach","owner":self.intent.owner,"correlation":self.intent.correlation}),
        )
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            let destination =
                Path::new(&destination).join(format!("retained-{}", self.intent.correlation));
            let _ = fs::create_dir_all(&destination);
            if let Ok(entries) = fs::read_dir(&self.intent.output_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_file()) {
                        let _ = fs::copy(entry.path(), destination.join(entry.file_name()));
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&self.intent.output_dir);
        fn release(path: &Path) {
            if path.is_dir() {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                if let Ok(children) = fs::read_dir(path) {
                    for child in children.flatten() {
                        release(&child.path());
                    }
                }
            }
        }
        for bundle in &self.bundles {
            release(bundle);
            let _ = fs::remove_dir_all(bundle);
        }
    }
}

fn completed(status: u32) -> Vec<Value> {
    vec![
        json!({"timestamp":1.0,"name":"start","context":{}}),
        json!({"timestamp":2.0,"name":"finish","context":{"status":status}}),
        json!({"timestamp":3.0,"name":"clean","context":{}}),
    ]
}

#[test]
#[ignore = "requires admitted PB CLI child fixture"]
fn jobspec_and_native_access_environment_are_exact_and_exclude_caller_sentinels() {
    let f = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .args(["--config", f.config.to_str().unwrap()])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TL3_TEST_CALLER_API_TOKEN", "synthetic-secret-sentinel")
        .env("TL3_TEST_CALLER_BENIGN", "synthetic-benign-sentinel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        json!({"op":"submit","request":f.request,"intent":f.intent})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let frames: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect();
    assert_eq!(
        frames.last().unwrap()["result"]["submission"]["kind"],
        "accepted",
        "{frames:?}"
    );
    let expected = json!({
        "PATH":"/usr/bin:/bin", "OMP_NUM_THREADS":"1", "MKL_NUM_THREADS":"1", "OPENBLAS_NUM_THREADS":"1",
        "TORCH_NUM_THREADS":"1", "NUMEXPR_NUM_THREADS":"1", "VECLIB_MAXIMUM_THREADS":"1", "BLIS_NUM_THREADS":"1",
        "OUT":f.intent.output_dir, "TL3_RUN_DIAGNOSTIC_DIR":f.intent.output_dir,
        "FLUX_SCRATCH":"/var/tmp/flux-vanilla", "TL3_RUN_BACKEND":"flux", "TL3_RUN_INSTANCE":f.intent.instance,
        "TL3_RUN_OWNER":f.intent.owner, "TL3_RUN_CORRELATION":f.intent.correlation,
        "TL3_RUN_REQUEST_DIGEST":f.intent.request_digest, "TL3_EXECUTION_BACKEND":"flux", "TL3_RUN_BACKEND_SESSION":"1",
        "TL3_RUN_BACKEND_CLI":env!("CARGO_BIN_EXE_tl3-run-backend"), "TL3_RUN_BACKEND_CONFIG":f.config,
    });
    for path in [
        f.root.path().join("jobspec.json"),
        f.intent.output_dir.join("original-jobspec.json"),
    ] {
        let spec: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(spec["attributes"]["system"]["environment"], expected);
    }
    let access: Value = serde_json::from_slice(
        &fs::read(f.root.path().join("native-access-environment.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        access,
        json!({"PATH":"/usr/bin:/bin","FLUX_URI":"local:///fixture"})
    );
    for key in ["TL3_TEST_CALLER_API_TOKEN", "TL3_TEST_CALLER_BENIGN"] {
        assert!(expected.get(key).is_none() && access.get(key).is_none());
    }
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join("environment-gate");
        fs::create_dir_all(&destination).unwrap();
        fs::write(
            destination.join("expected-job-environment.json"),
            expected.to_string(),
        )
        .unwrap();
        fs::write(
            destination.join("actual-native-access-environment.json"),
            access.to_string(),
        )
        .unwrap();
        fs::copy(
            f.root.path().join("jobspec.json"),
            destination.join("submitted-jobspec.json"),
        )
        .unwrap();
        fs::copy(
            f.intent.output_dir.join("original-jobspec.json"),
            destination.join("retained-original-jobspec.json"),
        )
        .unwrap();
    }
    // Demonstrate this producer models inheritance and ordered clearing independently.
    let args = fs::read_to_string(f.root.path().join("args"))
        .unwrap()
        .lines()
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let without_clear = args
        .iter()
        .filter(|a| a.as_str() != "--env=-*")
        .cloned()
        .collect::<Vec<_>>();
    let inherited = Command::new("/usr/bin/python3")
        .arg(f.root.path().join("jobspec.py"))
        .arg(f.root.path())
        .args(&without_clear)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TL3_TEST_CALLER_BENIGN", "synthetic-benign-sentinel")
        .output()
        .unwrap();
    assert!(
        inherited.status.success(),
        "{}",
        String::from_utf8_lossy(&inherited.stderr)
    );
    let unisolated: Value = serde_json::from_slice(&inherited.stdout).unwrap();
    assert_ne!(unisolated["attributes"]["system"]["environment"], expected);
    assert_eq!(
        unisolated["attributes"]["system"]["environment"]["TL3_TEST_CALLER_BENIGN"],
        "synthetic-benign-sentinel"
    );
}

#[test]
#[ignore = "requires admitted PB CLI child fixture"]
fn jobspec_submit_clears_propagated_rlimits() {
    let f = Fixture::new();
    // The submitting process carries a distinctive soft limit that Flux would propagate.
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("ulimit -S -n 777 && exec \"$0\" \"$@\"")
        .arg(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .args(["--config", f.config.to_str().unwrap()])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        json!({"op":"submit","request":f.request,"intent":f.intent})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let frames: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect();
    assert_eq!(
        frames.last().unwrap()["result"]["submission"]["kind"],
        "accepted",
        "{frames:?}"
    );
    let args = fs::read_to_string(f.root.path().join("args"))
        .unwrap()
        .lines()
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        args.iter().filter(|a| a.as_str() == "--rlimit=-*").count(),
        1,
        "{args:?}"
    );
    for path in [
        f.root.path().join("jobspec.json"),
        f.intent.output_dir.join("original-jobspec.json"),
    ] {
        let spec: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let options = &spec["attributes"]["system"]["shell"]["options"];
        assert!(options.is_object(), "{spec}");
        assert!(options.get("rlimit").is_none(), "{spec}");
    }
    // Demonstrate this producer models propagation: without the reset the sentinel leaks.
    let without_reset = args
        .iter()
        .filter(|a| a.as_str() != "--rlimit=-*")
        .cloned()
        .collect::<Vec<_>>();
    let propagated = Command::new("/bin/sh")
        .arg("-c")
        .arg("ulimit -S -n 777 && exec /usr/bin/python3 \"$@\"")
        .arg("jobspec-producer")
        .arg(f.root.path().join("jobspec.py"))
        .arg(f.root.path())
        .args(&without_reset)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        propagated.status.success(),
        "{}",
        String::from_utf8_lossy(&propagated.stderr)
    );
    let leaked: Value = serde_json::from_slice(&propagated.stdout).unwrap();
    assert_eq!(
        leaked["attributes"]["system"]["shell"]["options"]["rlimit"]["nofile"],
        777
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn jobspec_admits_declared_memory_as_rounded_gib_leaves_and_preserves_original() {
    for (bytes, leaves) in [(1_u64, 1_u64), (1 << 30, 1), ((1 << 30) + 1, 2)] {
        let mut f = Fixture::new();
        f.request.reservation.memory_bytes = bytes.try_into().unwrap();
        f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
        assert_eq!(f.submit()["kind"], "accepted");
        let spec: Value = serde_json::from_slice(
            &fs::read(f.intent.output_dir.join("original-jobspec.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(spec["resources"][0]["type"], "slot");
        assert_eq!(
            spec["resources"][0]["with"],
            json!([
                {"type":"core","count":2}, {"type":"memory","count":leaves}
            ])
        );
        assert_eq!(
            spec["attributes"]["user"]["memory_bytes"],
            bytes.to_string()
        );
        assert_eq!(
            spec["attributes"]["system"]["environment"]["OMP_NUM_THREADS"],
            "1"
        );
        let environment = &spec["attributes"]["system"]["environment"];
        for key in ["OUT", "TL3_RUN_DIAGNOSTIC_DIR"] {
            assert_eq!(environment[key], f.intent.output_dir.to_str().unwrap());
        }
        assert_eq!(environment["TL3_RUN_CORRELATION"], f.intent.correlation);
        assert_eq!(
            spec["tasks"][0]["command"][0],
            f.request.runner.executable.to_str().unwrap()
        );
        // Native original contains memory too, so recovery compares the whole admitted shape.
        fs::write(f.root.path().join("events"), "").unwrap();
        let frames = f.attach();
        assert_ne!(frames.last().unwrap()["kind"], "error", "{frames:?}");
        let mut altered = spec;
        altered["resources"][0]["with"][1]["count"] = json!(leaves + 1);
        fs::write(f.root.path().join("jobspec.json"), altered.to_string()).unwrap();
        assert_eq!(
            f.attach().last().unwrap()["result"]["observation"]["kind"],
            "unknown"
        );
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn malformed_memory_jobspec_shape_never_dispatches() {
    for replacement in [
        "spec['resources'][0]['count']=2",
        "spec['resources'][0]['with'][0]['count']=0",
        "spec['resources'][0]['with'].append({'type':'memory','count':1})",
        "spec['resources'][0]['with'][0]['count']=3",
    ] {
        let f = Fixture::new();
        let path = f.root.path().join("jobspec.py");
        let source = fs::read_to_string(&path).unwrap().replace(
            "text=json.dumps(spec);",
            &format!("{replacement}\ntext=json.dumps(spec);"),
        );
        fs::write(path, source).unwrap();
        let frames = f.call(json!({"op":"submit","request":f.request,"intent":f.intent}));
        assert_eq!(frames.last().unwrap()["kind"], "error", "{frames:?}");
        assert!(!f.calls().contains("job submit "));
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn resource_kill_without_oom_stderr_is_infrastructure_only_after_cleanup() {
    for status in [137 * 256, 143 * 256, 9, 15] {
        let f = Fixture::new();
        assert_eq!(f.submit()["kind"], "accepted");
        f.report(ReportOutcome::TestsFailed, false);
        f.events(&completed(status));
        assert_eq!(
            f.attach().last().unwrap()["result"]["observation"]["kind"],
            "unknown"
        );
        f.report(ReportOutcome::TestsFailed, true);
        let frames = f.attach();
        let terminal = &frames.last().unwrap()["result"]["observation"]["terminal"];
        assert_eq!(terminal["ending"], "infra-lost", "{frames:?}");
        assert!(
            terminal["reason"]
                .as_str()
                .unwrap()
                .contains("infrastructure/resource")
        );
        assert!(
            terminal["reason"]
                .as_str()
                .unwrap()
                .contains("not inferred")
        );
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn acceptance_unknown_never_resubmits_and_original_jobspec_recovers_one_match() {
    let f = Fixture::new();
    fs::write(f.root.path().join("mode"), "unknown").unwrap();
    assert_eq!(f.submit()["kind"], "unknown");
    assert_eq!(f.submit()["kind"], "unknown");
    assert_eq!(
        f.calls()
            .lines()
            .filter(|line| line.starts_with("job submit "))
            .count(),
        1
    );
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    assert!(f.state().binding.is_none());
    // The controller's attach recovers the binding before the runner can observe placement.
    f.attach();
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
    assert!(f.calls().contains("job info --original fABC jobspec"));
    assert_eq!(
        f.calls()
            .lines()
            .filter(|line| line.starts_with("job submit "))
            .count(),
        1
    );
    f.report(ReportOutcome::Passed, true);
    f.events(&completed(0));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "passed",
        "{frames:?}"
    );
    assert!(f.calls().contains("job info --original fABC jobspec"));
    let state: Value = serde_json::from_slice(
        &fs::read(
            f.options
                .journal_root
                .join(&f.intent.correlation)
                .join("state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(state["binding"]["id"]["uri"], "local:///fixture");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn orphan_zero_two_and_conflicting_matches_remain_unknown() {
    for matches in [0, 2] {
        let f = Fixture::new();
        fs::write(f.root.path().join("mode"), "unknown").unwrap();
        f.submit();
        fs::write(f.root.path().join("jobs.json"), json!({"jobs":(0..matches).map(|_|json!({"name":f.intent.correlation,"jobid":"fABC"})).collect::<Vec<_>>()} ).to_string()).unwrap();
        assert_eq!(
            f.attach().last().unwrap()["result"]["observation"]["kind"],
            "unknown"
        );
        assert_eq!(
            f.calls()
                .lines()
                .filter(|line| line.starts_with("job submit "))
                .count(),
            1
        );
    }
    let f = Fixture::new();
    fs::write(f.root.path().join("mode"), "unknown").unwrap();
    f.submit();
    let path = f.root.path().join("jobspec.json");
    let mut spec: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    spec["attributes"]["user"]["tree_digest"] = json!("different-tree");
    fs::write(path, spec.to_string()).unwrap();
    assert_eq!(
        f.attach().last().unwrap()["result"]["observation"]["kind"],
        "unknown"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn replay_deduplicates_retained_prefix_and_transport_error_stays_unknown() {
    let f = Fixture::new();
    f.submit();
    f.report(ReportOutcome::Passed, true);
    f.events(&completed(0));
    fs::write(f.root.path().join("mode"), "interrupted").unwrap();
    let first = f.attach();
    assert_eq!(
        first.last().unwrap()["result"]["observation"]["kind"],
        "unknown"
    );
    assert_eq!(
        first
            .iter()
            .filter(|frame| frame["kind"] == "event")
            .count(),
        1
    );
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    let second = f.attach();
    assert_eq!(
        second
            .iter()
            .filter(|frame| frame["kind"] == "event")
            .count(),
        2
    );
    assert_eq!(
        second.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "passed",
        "{second:?}"
    );
    assert_eq!(
        f.attach()
            .iter()
            .filter(|frame| frame["kind"] == "event")
            .count(),
        0
    );
    assert!(!f.calls().contains("job wait "));
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn posix_and_five_endings_require_semantic_and_cleanup_evidence() {
    assert_eq!(
        api::decode_wait_status(256),
        WaitStatus::Exited { raw: 256, code: 1 }
    );
    assert_eq!(
        api::decode_wait_status(15),
        WaitStatus::Signalled {
            raw: 15,
            signal: 15,
            core_dumped: false
        }
    );
    assert_eq!(
        api::decode_wait_status(139),
        WaitStatus::Signalled {
            raw: 139,
            signal: 11,
            core_dumped: true
        }
    );
    for (outcome, status, expected) in [
        (ReportOutcome::Passed, 0, "passed"),
        (ReportOutcome::TestsFailed, 256, "failed"),
    ] {
        let f = Fixture::new();
        f.submit();
        f.report(outcome, true);
        f.events(&completed(status));
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
            expected,
            "{frames:?}"
        );
        let receipt: BlobRef = serde_json::from_value(
            frames.last().unwrap()["result"]["observation"]["terminal"]["receipt"].clone(),
        )
        .unwrap();
        let bytes = fs::read(&receipt.path).unwrap();
        assert_eq!(digest(&bytes), receipt.sha256);
        let receipt: Receipt = serde_json::from_slice(&bytes).unwrap();
        assert!(
            receipt
                .evidence
                .iter()
                .any(|blob| blob.path.ends_with("eventlog.jsonl"))
        );
        for reference in receipt.evidence {
            assert_eq!(digest(&fs::read(reference.path).unwrap()), reference.sha256);
        }
    }
    let f = Fixture::new();
    f.submit();
    f.events(&completed(0));
    assert_eq!(
        f.attach().last().unwrap()["result"]["observation"]["kind"],
        "unknown"
    );
    let f = Fixture::new();
    f.submit();
    f.report(ReportOutcome::TestsFailed, true);
    f.events(&completed(15));
    assert_eq!(
        f.attach().last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "infra-lost"
    );
    let mut f = Fixture::new();
    f.request.reservation.memory_guarantee = MemoryGuarantee::RequireAdmission;
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    assert_eq!(f.submit()["terminal"]["ending"], "refused");
    assert!(!f.calls().contains("job submit "));
    let f = Fixture::new();
    f.submit();
    f.report(ReportOutcome::EnvironmentLost, true);
    f.events(&[json!({"name":"exception","context":{"type":"exec","severity":0,"note":"owned execution lost"}}), json!({"name":"clean"})]);
    assert_eq!(
        f.attach().last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "infra-lost"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cancel_ack_needs_native_cancel_and_stopped_scope_and_finish_race_keeps_original() {
    for finish_first in [false, true] {
        let f = Fixture::new();
        f.submit();
        f.report(ReportOutcome::Passed, true);
        let ack = f
            .call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_eq!(
            ack.last().unwrap()["result"]["acknowledgement"]["acknowledged"],
            true
        );
        assert_eq!(
            ack.last().unwrap()["result"]["acknowledgement"]["terminal"],
            Value::Null
        );
        let cancel = json!({"name":"exception","context":{"type":"cancel","severity":0,"note":"owned cancellation"}});
        let mut events = completed(0);
        events.insert(if finish_first { 2 } else { 1 }, cancel);
        f.events(&events);
        assert_eq!(
            f.attach().last().unwrap()["result"]["observation"]["terminal"]["ending"],
            if finish_first { "passed" } else { "cancelled" }
        );
    }
    let f = Fixture::new();
    f.submit();
    f.report(ReportOutcome::Passed, false);
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    f.events(&[
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"clean"}),
    ]);
    assert_eq!(
        f.attach().last().unwrap()["result"]["observation"]["kind"],
        "unknown"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn probe_reports_busy_incompatible_unknown_and_never_ram_admission() {
    let mut f = Fixture::new();
    let placement = f.request.runtime.placement();
    let command = |r: &Reservation| json!({"op":"probe","reservation":r,"placement":placement});
    assert_eq!(
        f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"],
        "available"
    );
    fs::write(
        f.root.path().join("free.json"),
        json!({"version":1,"execution":{"R_lite":[]}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"],
        "busy"
    );
    f.request.reservation.cpus = 8.try_into().unwrap();
    assert_eq!(
        f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"],
        "incompatible"
    );
    fs::write(f.root.path().join("all.json"), "bad-json").unwrap();
    assert_eq!(
        f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"],
        "unknown"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn quiet_native_cli_observer_stops_and_reaps_without_cancelling_job() {
    let f = Fixture::new();
    f.submit();
    f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":f.ownership()}));
    fs::write(f.root.path().join("mode"), "quiet").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .args(["--config", f.config.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})
    )
    .unwrap();
    wait_until("quiet watcher started", Duration::from_secs(5), || {
        f.root.path().join("watching").exists()
    });
    let stopped = Instant::now();
    writeln!(stdin, "{{\"op\":\"stop_watch\"}}").unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        stopped.elapsed() < stop_watch_deadline(),
        "stop_watch did not stop the observer well before the native command deadline"
    );
    check_budget("quiet watcher stop_watch", stopped, Duration::from_secs(2));
    let frames: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{frames:?}"
    );
    assert!(!f.calls().lines().any(|line| line.starts_with("cancel ")));
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generation_slash_is_not_a_path_and_original_spec_allows_restart() {
    assert!(api::generation(
        "fv1-model/61f47a7f95af46d794ca89c3a71f0654"
    ));
    assert!(!api::generation("../../host"));
    let f = Fixture::new();
    f.submit();
    let options = f.options.clone();
    fs::write(&options.instance_file, "local:///replacement\n").unwrap();
    fs::write(&f.config, serde_json::to_vec(&options).unwrap()).unwrap();
    assert_ne!(f.attach().last().unwrap()["kind"], "error");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn separate_process_attach_cancel_and_late_terminal_preserve_journal_state() {
    let f = Fixture::new();
    f.submit();
    f.retain_fixture_allocation();
    f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":f.ownership()}));
    fs::write(f.root.path().join("mode"), "quiet").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .args(["--config", f.config.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})
    )
    .unwrap();
    wait_until(
        "separate attach watcher started",
        Duration::from_secs(5),
        || f.root.path().join("watching").exists(),
    );
    let cancel =
        f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(
        cancel.last().unwrap()["result"]["acknowledgement"]["acknowledged"],
        true
    );
    writeln!(stdin, "{{\"op\":\"stop_watch\"}}").unwrap();
    drop(stdin);
    assert!(child.wait_with_output().unwrap().status.success());
    assert_eq!(
        f.calls()
            .lines()
            .filter(|line| line.starts_with("cancel "))
            .count(),
        1
    );
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    f.report(ReportOutcome::Passed, true);
    f.events(&completed(0));
    let first = f.attach();
    let receipt = first.last().unwrap()["result"]["observation"]["terminal"]["receipt"].clone();
    assert_ne!(receipt, Value::Null, "{first:?}");
    let path = f
        .options
        .journal_root
        .join(&f.intent.correlation)
        .join("state.json");
    let state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(state["cancel_requested"], true);
    assert_eq!(state["binding"]["id"]["job"], "fABC");
    assert_eq!(state["events"].as_array().unwrap().len(), 3);
    let second = f.attach();
    assert_eq!(
        second.last().unwrap()["result"]["observation"]["terminal"]["receipt"],
        receipt
    );
}

#[test]
fn optional_reviewed_sha_remains_distinct_in_wire_and_request_digest() {
    let absent = Suite::DaytimeChecks {
        plan_digest: "a".repeat(64),
        base: "b".repeat(40),
        head: "d".repeat(40),
        reviewed: None,
    };
    let present = Suite::DaytimeChecks {
        plan_digest: "a".repeat(64),
        base: "b".repeat(40),
        head: "d".repeat(40),
        reviewed: Some("c".repeat(40)),
    };
    let absent_json = serde_json::to_value(&absent).unwrap();
    let present_json = serde_json::to_value(&present).unwrap();
    assert_eq!(absent_json["reviewed"], Value::Null);
    assert_eq!(present_json["reviewed"], "c".repeat(40));
    assert_ne!(
        digest(&serde_json::to_vec(&absent).unwrap()),
        digest(&serde_json::to_vec(&present).unwrap())
    );
    let restored: Suite = serde_json::from_value(absent_json).unwrap();
    assert_eq!(restored, absent);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn all_attach_native_phases_stop_and_reap_without_native_cancel() {
    for mode in ["quiet-jobs", "quiet-info", "quiet-collect"] {
        let f = Fixture::new();
        if mode == "quiet-jobs" {
            fs::write(f.root.path().join("mode"), "unknown").unwrap();
        }
        f.submit();
        f.events(&completed(0));
        fs::write(f.root.path().join("mode"), mode).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
            .args(["--config", f.config.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})
        )
        .unwrap();
        wait_until(
            &format!("phase {mode} reached"),
            Duration::from_secs(5),
            || f.root.path().join("watching").exists(),
        );
        let start = Instant::now();
        writeln!(stdin, "{{\"op\":\"stop_watch\"}}").unwrap();
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(
            start.elapsed() < stop_watch_deadline(),
            "phase {mode} stop_watch did not stop the observer well before the native command deadline"
        );
        check_budget(
            &format!("phase {mode} stop_watch"),
            start,
            Duration::from_secs(2),
        );
        assert!(!output.stdout.is_empty());
        assert!(!f.calls().lines().any(|call| call.starts_with("cancel ")));
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn collector_exact_frame_and_typed_terminal_payload_verify_all_links() {
    let mut f = Fixture::new();
    f.request.suite = Suite::DaytimeChecks {
        plan_digest: "a".repeat(64),
        base: "b".repeat(40),
        head: "c".repeat(40),
        reviewed: None,
    };
    let prepared = f.call(json!({"op":"prepare_intent","owner":f.intent.owner,"correlation":f.intent.correlation,"request":f.request}));
    f.intent =
        serde_json::from_value(prepared.last().unwrap()["result"]["intent"].clone()).unwrap();
    f.submit();
    f.report(ReportOutcome::ChecksCompleted, true);
    let refusal = "proof row diagnostic ".repeat(2000);
    let report = json!({"schema":"tl3.checks.v3","effect_key":"fixture","base":"b".repeat(40),"head":"c".repeat(40),"reviewed":null,"results":[{
        "name":"complete refusal","kind":"demonstrating","verdict":"infrastructure-failure","custody":"in-process","tests":[{
            "test":"tests/test_subject.py::test_value","base":"uncollected","head":"uncollected","pre_existing":false,"passed":false,
            "message":"bounded summary","proof_refusal":refusal}]}]});
    let out = &f.intent.output_dir;
    let bytes = format!("trusted host prefix\nTL3_CHECKS {report}\n").into_bytes();
    let stdout = api::durable::blob(&out.join("raw-checks-stdout"), &bytes).unwrap();
    let cleanup = api::durable::blob(
        &out.join("cleanup.txt"),
        &fs::read(out.join("cleanup.txt")).unwrap(),
    )
    .unwrap();
    let frames = f.call(json!({"op":"collect_runner","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{
        "run":f.ownership().run,"stdout":stdout,"runtime_binding":null,"cleanup":cleanup}}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let retained: BlobRef =
        serde_json::from_value(frames.last().unwrap()["result"]["payload"].clone()).unwrap();
    assert_eq!(
        fs::read(&retained.path).unwrap(),
        report.to_string().as_bytes()
    );
    f.events(&completed(0));
    let terminal = f.attach().last().unwrap()["result"]["observation"]["terminal"].clone();
    assert_eq!(terminal["ending"], "passed", "{terminal}");
    let payload = f.call(json!({"op":"daytime_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(
        payload.last().unwrap()["result"]["report"],
        report,
        "{payload:?}"
    );
    fs::write(retained.path, b"{}").unwrap();
    let corrupt = f.call(json!({"op":"daytime_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(corrupt.last().unwrap()["kind"], "error");
    for raw in [
        "no frame\n".to_owned(),
        "TL3_CHECKS {}\n".to_owned(),
        format!("TL3_CHECKS {report}\nTL3_CHECKS {report}\n"),
    ] {
        let stdout = api::durable::blob(&out.join("invalid-stdout"), raw.as_bytes()).unwrap();
        let invalid = f.call(json!({"op":"collect_runner","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{
            "run":f.ownership().run,"stdout":stdout,"runtime_binding":null,"cleanup":null}}));
        assert_eq!(invalid.last().unwrap()["kind"], "error", "{invalid:?}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn exact_original_spec_rejects_resource_placement_and_duration_changes() {
    for field in ["resources", "placement", "duration"] {
        let f = Fixture::new();
        fs::write(f.root.path().join("mode"), "unknown").unwrap();
        f.submit();
        let path = f.root.path().join("jobspec.json");
        let mut spec: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match field {
            "resources" => spec["resources"][0]["count"] = json!(100),
            "placement" => spec["attributes"]["system"]["constraints"] = json!("host:sparky"),
            _ => spec["attributes"]["system"]["duration"] = json!("999s"),
        }
        fs::write(path, spec.to_string()).unwrap();
        assert_eq!(
            f.attach().last().unwrap()["result"]["observation"]["kind"],
            "unknown"
        );
    }
}

impl Fixture {
    fn podman(&mut self, mode: ContainerMode, prerequisites: bool) {
        let p = self.root.path();
        let image_map = p.join("image-map.json");
        fs::write(&image_map, json!({"profiles":[{"profile":"fixture","registry_ref":format!("fixture.invalid/test@sha256:{}", "a".repeat(64)),
            "manifest_digest":format!("sha256:{}", "a".repeat(64)),"config_digest":format!("sha256:{}", "b".repeat(64)),"platform":"linux/amd64"}]}).to_string()).unwrap();
        let image_inspect = p.join("image-inspect.json");
        fs::write(
            &image_inspect,
            json!({"Id":format!("sha256:{}", "b".repeat(64)),"Architecture":"amd64","Os":"linux"})
                .to_string(),
        )
        .unwrap();
        let source = p.join("host-source");
        let trusted = source.join("lib/tl3_harness");
        fs::create_dir_all(&trusted).unwrap();
        fs::write(trusted.join("runner.py"), b"# trusted runner fixture\n").unwrap();
        fs::write(
            trusted.join("runtime.py"),
            b"# trusted imported module fixture\n",
        )
        .unwrap();
        fs::write(
            source.join("host-collector"),
            b"#!/usr/bin/python3.14 -I\n# trusted collector fixture\n",
        )
        .unwrap();
        fs::write(
            source.join("cleanup-helper"),
            r#"#!/usr/bin/python3 __PYTHON_FLAGS__
import json,sys,pathlib,time
__SCRATCH_FIXTURE__
facts=json.load(sys.stdin)
if facts['schema']=='tl3.scratch_cleanup_input.v1':
    fixture_scratch(facts)
    sys.exit(0)
if facts['schema']=='tl3.job_unit_absence_input.v1':
    output=pathlib.Path('/mnt/shared/flux-out/tl3')/facts['owner']['correlation']
    control=output/'unit-control.json'
    control=json.loads(control.read_text()) if control.exists() else {}
    now=time.monotonic_ns()
    guard={'exit_code':0,'stdout':facts['run']['instance'].split('/')[-1]+'\n','stderr':''}
    result={'schema':'tl3.job_unit_absence.v1','run':facts['run'],'owner':facts['owner'],'request_digest':facts['request_digest'],
      'host':facts['host'],'nonce':facts['nonce'],
      'path':'/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-'+facts['run']['job']+':sys.service',
      'control':{'path':'/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice','observation':{'kind':'directory'}},
      'generation_before':guard,'generation_after':guard,'observation':{'kind':'absent','errno':2},
      'started_monotonic_ns':now,'finished_monotonic_ns':now+1}
    result.update(control.get('replace',{}))
    if result.get('control') is None: result.pop('control',None)
    print(json.dumps(result),flush=True)
    sys.exit(control.get('exit_code',0))
owned=facts['ownership']
root=pathlib.Path('/mnt/shared/flux-out/tl3')/owned['owner']['correlation']
(root/('helper-called-'+owned['execution'])).write_text(json.dumps(facts))
with (root/('helper-attempts-'+owned['execution'])).open('a') as attempts:
    attempts.write('attempt\n')
control=root/('helper-control-'+owned['execution']+'.json')
control=json.loads(control.read_text()) if control.exists() else {}
time.sleep(control.get('delay',0))
print((root/('trusted-stop-'+owned['execution']+'.json')).read_text(),flush=True)
sys.exit(control.get('exit_code',0))
"#
            .replace("__PYTHON_FLAGS__", PYTHON_NO_BYTECODE)
            .replace("__SCRATCH_FIXTURE__", include_str!("python/fx_scratch_helper.py")),
        )
        .unwrap();
        let files = ["lib/tl3_harness/runner.py", "lib/tl3_harness/runtime.py"]
            .into_iter()
            .map(|path| {
                let raw = fs::read(source.join(path)).unwrap();
                CodeFile {
                    path: path.into(),
                    sha256: digest(&raw),
                    bytes: raw.len() as u64,
                }
            })
            .collect();
        fs::write(
            source.join("code.json"),
            serde_json::to_vec(&CodeInventory {
                interpreter: "/usr/bin/python3.14".into(),
                files,
            })
            .unwrap(),
        )
        .unwrap();
        let input = PrepareBundleInput {
            source_root: source,
            code_inventory: "code.json".into(),
            collector: "host-collector".into(),
            cleanup: "cleanup-helper".into(),
            trusted_root: "lib/tl3_harness".into(),
            producer_commit: "a".repeat(40),
            container_interpreter: "/usr/bin/python3.12".into(),
            cleanup_arguments: vec!["--fixture-cleanup".into()],
            image_inventory: image_map,
            image_inspect,
            profile: "fixture".into(),
            mode,
            prerequisites: ModePrerequisites {
                no_cgroups_supported: prerequisites,
                init_available: prerequisites,
                user_scope_supported: prerequisites,
            },
        };
        let frames = self.call(json!({"op":"prepare_runtime_bundle","input":input}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        let bundle: BlobRef =
            serde_json::from_value(frames.last().unwrap()["result"]["bundle"].clone()).unwrap();
        self.bundles.push(bundle.path.parent().unwrap().to_owned());
        let prepared = self.call(json!({"op":"prepare_runtime","bundle":bundle.path,"reservation":self.request.reservation,"arguments":["--fixture"]}));
        assert_eq!(prepared.last().unwrap()["kind"], "result", "{prepared:?}");
        let prepared: PreparedRuntime =
            serde_json::from_value(prepared.last().unwrap()["result"]["prepared"].clone()).unwrap();
        assert!(
            matches!(&prepared.runtime.runtime, ExecutionRuntime::RootlessPodman { interpreter, .. } if interpreter == Path::new("/usr/bin/python3.12"))
        );
        assert_eq!(
            prepared.runtime.cleanup.as_ref().unwrap().arguments,
            ["--fixture-cleanup"]
        );
        self.request.runner = prepared.runner;
        self.request.runtime = prepared.runtime;
        self.request.scratch_root = prepared.scratch_root;
        self.request
            .environment
            .insert("HOME".into(), "/home/rob".into());
        self.request
            .environment
            .insert("XDG_RUNTIME_DIR".into(), "/run/user/1000".into());
        self.intent.request_digest = digest(&serde_json::to_vec(&self.request).unwrap());
    }

    fn container_ownership(&self) -> RuntimeOwnership {
        let mut owned = self.ownership();
        let mode = match self.request.runtime.runtime {
            ExecutionRuntime::RootlessPodman { mode, .. } => mode,
            _ => unreachable!(),
        };
        let unit = format!("tl3-{}-fABC-owned.scope", self.intent.correlation);
        let scope = ScopeIdentity {
            cgroup: format!("/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}"),
            unit,
            invocation_id: "c".repeat(32),
        };
        let frames = self.call(json!({"op":"begin_container_launch","owner":self.intent.owner,"correlation":self.intent.correlation,"launcher":owned}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        let launch: ContainerLaunchIntent =
            serde_json::from_value(frames.last().unwrap()["result"]["launch"].clone()).unwrap();
        owned.container = Some(ContainerOwnership {
            name: launch.name,
            mode,
            id: "d".repeat(64),
            labels: launch.labels,
            scope: (mode == ContainerMode::OwnScope).then_some(scope),
        });
        owned
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn selected_mode_refuses_missing_prerequisites_without_fallback() {
    for mode in [ContainerMode::InJob, ContainerMode::OwnScope] {
        let mut f = Fixture::new();
        f.podman(mode, false);
        assert_eq!(f.submit()["terminal"]["ending"], "refused");
        assert!(!f.calls().contains("job submit "));
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn own_scope_cleanup_requires_exact_generation_absence_and_unpopulated_scope() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.container_ownership();
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let container = owned.container.as_ref().unwrap();
    let scope = container.scope.as_ref().unwrap();
    let mut facts = CleanupEvidence {
        schema: "tl3.cleanup_evidence.v1".into(),
        pending_launch: None,
        ownership: owned.clone(),
        launcher: ProcessStop::Absent,
        process_group_probe_errno: Some(3),
        container: Some(ContainerStop {
            name: container.name.clone(),
            name_exists_exit: 1,
            sweep_exit: 0,
            remaining_ids: vec![],
            id: container.id.clone(),
            remove_exit: 0,
            exists_exit: 1,
            sweep_ids: vec![container.id.clone()],
            scope: Some(ScopeStop {
                identity: scope.clone(),
                observed_invocation_id: Some(scope.invocation_id.clone()),
                cgroup_events: Some("populated 1\nfrozen 0\n".into()),
                cgroup_procs: String::new(),
                active_state: "inactive".into(),
            }),
        }),
    };
    for case in ["populated", "wrong-generation", "podman-error", "stopped"] {
        let stop = facts.container.as_mut().unwrap();
        stop.exists_exit = if case == "podman-error" { 125 } else { 1 };
        let scope_stop = stop.scope.as_mut().unwrap();
        scope_stop.cgroup_events = Some(
            if case == "populated" {
                "populated 1\n"
            } else {
                "populated 0\n"
            }
            .into(),
        );
        scope_stop.observed_invocation_id = Some(if case == "wrong-generation" {
            "e".repeat(32)
        } else {
            scope.invocation_id.clone()
        });
        let reference = api::durable::blob(
            &f.intent.output_dir.join("typed-cleanup.json"),
            &serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
        let frames = f.call(json!({"op":"retain_cleanup","owner":f.intent.owner,"correlation":f.intent.correlation,"evidence":reference}));
        assert_eq!(
            frames.last().unwrap()["result"]["cleanup"]["stopped"],
            case == "stopped",
            "{case}: {frames:?}"
        );
    }
    let mut changed = owned;
    changed.container.as_mut().unwrap().id = "f".repeat(64);
    let conflict = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":changed}));
    assert_eq!(conflict.last().unwrap()["kind"], "error");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn own_scope_runtime_mapping_compares_golden_dl380_sets_and_effective_cap() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.request.reservation.cpus = 4.try_into().unwrap();
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    f.submit();
    let owned = f.container_ownership();
    f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    let native = serde_json::to_vec(&json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":"21-23,30"}}],"nodelist":["dl380g10"]},
        "scheduling":{"graph":{"nodes":std::iter::once(json!({"metadata":{"type":"node","rank":0,"id":0,"name":"dl380g10"}}))
            .chain([21,22,23,30].into_iter().map(|id| json!({"metadata":{"type":"core","rank":0,"id":id,"properties":{"os-pus":format!("{id},{}",id+40)}}}))).collect::<Vec<_>>()}}})).unwrap();
    fs::write(f.root.path().join("all.json"), &native).unwrap();
    let allocation = api::durable::blob(&f.intent.output_dir.join("R.json"), &native).unwrap();
    let topology = api::durable::blob(
        &f.intent.output_dir.join("topology.json"),
        &serde_json::to_vec(&TopologyMapping {
            run: owned.run.clone(),
            request_digest: f.intent.request_digest.clone(),
            ranks: vec![RankMapping {
                rank: 0,
                host: "dl380g10".into(),
                cores: [21, 22, 23, 30]
                    .into_iter()
                    .map(|i| CoreMapping {
                        flux_core: i,
                        os_pus: format!("{i},{}", i + 40),
                    })
                    .collect(),
            }],
        })
        .unwrap(),
    )
    .unwrap();
    let scope = owned.container.as_ref().unwrap().scope.as_ref().unwrap();
    let mut facts = RuntimeBindingInput {
        execution: owned.execution.clone(),
        container_id: owned
            .container
            .as_ref()
            .map(|container| container.id.clone()),
        run: owned.run.clone(),
        request_digest: f.intent.request_digest.clone(),
        allocation,
        topology,
        host: ProcessPlacement {
            rank: 0,
            host: "dl380g10".into(),
            allowed_cpus: "21-23,30,61-63,70".into(),
            cgroup: "0::/user.slice/native-broker".into(),
        },
        container: Some(ProcessPlacement {
            rank: 0,
            host: "dl380g10".into(),
            allowed_cpus: "21-23,30,61-63,70".into(),
            cgroup: format!("0::{}", scope.cgroup),
        }),
        container_scope: Some(ContainerScopeFacts {
            unit: scope.unit.clone(),
            invocation_id: scope.invocation_id.clone(),
            cgroup: format!("0::{}", scope.cgroup),
            cpuset_effective: "21-23,30,61-63,70".into(),
            memory_max: Some(1_073_741_824),
            memory_swap_max: Some(0),
            container_id: owned.container.as_ref().unwrap().id.clone(),
        }),
    };
    let allocation = AllocationInput {
        run: facts.run.clone(),
        request_digest: facts.request_digest.clone(),
        allocation: facts.allocation.clone(),
        topology: facts.topology.clone(),
        host: facts.host.clone(),
    };
    let granted = f.call(json!({"op":"validate_allocation","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":allocation}));
    assert_eq!(
        granted.last().unwrap()["result"]["grant"]["os_pus"],
        json!([21, 22, 23, 30, 61, 62, 63, 70]),
        "{granted:?}"
    );
    let context = f.call(json!({"op":"launch_context","owner":f.intent.owner,"correlation":f.intent.correlation,"job":"fABC"}));
    assert_eq!(
        context.last().unwrap()["result"]["context"]["runtime_identity"]["image"]["profile"],
        "fixture"
    );
    assert_eq!(
        context.last().unwrap()["result"]["context"]["placement"]["os_pus"],
        json!([21, 22, 23, 30, 61, 62, 63, 70])
    );
    let frames = f.call(json!({"op":"validate_runtime_binding","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":facts}));
    assert_eq!(
        frames.last().unwrap()["result"]["qualification"]["qualified"],
        true,
        "{frames:?}"
    );
    facts.container_scope.as_mut().unwrap().memory_max = None;
    let frames = f.call(json!({"op":"validate_runtime_binding","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":facts}));
    assert_eq!(
        frames.last().unwrap()["result"]["qualification"]["qualified"],
        false
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn in_job_sdexec_requires_exact_native_job_unit_and_host_coordinates() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    f.request.reservation.cpus = 1.try_into().unwrap();
    f.request.reservation.parallelism = 1.try_into().unwrap();
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    fs::write(f.root.path().join("submit-id"), "fAxT19DuQT\n").unwrap();
    f.submit();
    let mut owned = f.ownership();
    owned.run.job = "fAxT19DuQT".into();
    let frames = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":owned}));
    let launch: ContainerLaunchIntent =
        serde_json::from_value(frames.last().unwrap()["result"]["launch"].clone()).unwrap();
    owned.container = Some(ContainerOwnership {
        mode: ContainerMode::InJob,
        id: "d".repeat(64),
        name: launch.name,
        labels: launch.labels,
        scope: None,
    });
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let native = serde_json::to_vec(&json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":"0"}}],"nodelist":["dl380g10"]}})).unwrap();
    let allocation = api::durable::blob(&f.intent.output_dir.join("R.json"), &native).unwrap();
    let topology = api::durable::blob(
        &f.intent.output_dir.join("topology.json"),
        &serde_json::to_vec(&TopologyMapping {
            run: owned.run.clone(),
            request_digest: f.intent.request_digest.clone(),
            ranks: vec![RankMapping {
                rank: 0,
                host: "dl380g10".into(),
                cores: vec![CoreMapping {
                    flux_core: 0,
                    os_pus: "0,40".into(),
                }],
            }],
        })
        .unwrap(),
    )
    .unwrap();
    let exact = "/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fAxT19DuQT:sys.service";
    let sibling = exact.replace("fAxT19DuQT", "fABC");
    let foreign = exact.replace("flux-jobs.slice", "foreign.slice");
    let suffix_collision = format!("{exact}-foreign");
    for (name, host, container, expected) in [
        ("retained-private-namespace-root", exact, "/", false),
        ("host-coordinates", exact, exact, true),
        ("contained", exact, &format!("{exact}/payload"), true),
        ("sibling-container", exact, sibling.as_str(), false),
        (
            "sibling-host-and-container",
            sibling.as_str(),
            sibling.as_str(),
            false,
        ),
        ("foreign-slice", foreign.as_str(), foreign.as_str(), false),
        (
            "slice-root",
            "/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice",
            "/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice",
            false,
        ),
        ("namespace-root", "/", "/", false),
        (
            "unit-prefix-collision",
            exact,
            suffix_collision.as_str(),
            false,
        ),
    ] {
        let process = |path: &str| json!({"rank":0,"host":"dl380g10","allowed_cpus":"0,40","cgroup":format!("0::{path}\n")});
        let facts = json!({"run":owned.run,"execution":owned.execution,
            "request_digest":f.intent.request_digest,"container_id":owned.container.as_ref().unwrap().id,
            "allocation":allocation,"topology":topology,"host":process(host),
            "container":process(container),"container_scope":null});
        let frames = f.call(json!({"op":"validate_runtime_binding","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":facts}));
        assert_eq!(
            frames.last().unwrap()["result"]["qualification"]["qualified"],
            expected,
            "{name}: {frames:?}"
        );
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cancellation_uses_independent_cleanup_without_runner_report_after_wrapper_loss() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    f.retain_fixture_allocation();
    let owned = f.container_ownership();
    let retained = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    assert_eq!(retained.last().unwrap()["kind"], "result");
    let container = owned.container.as_ref().unwrap();
    let scope = container.scope.as_ref().unwrap();
    let facts = CleanupEvidence {
        schema: "tl3.cleanup_evidence.v1".into(),
        pending_launch: None,
        ownership: owned.clone(),
        launcher: ProcessStop::Absent,
        process_group_probe_errno: Some(3),
        container: Some(ContainerStop {
            name: container.name.clone(),
            name_exists_exit: 1,
            sweep_exit: 0,
            remaining_ids: vec![],
            id: container.id.clone(),
            remove_exit: 0,
            exists_exit: 1,
            sweep_ids: vec![container.id.clone()],
            scope: Some(ScopeStop {
                identity: scope.clone(),
                observed_invocation_id: None,
                cgroup_events: None,
                cgroup_procs: String::new(),
                active_state: "not-found".into(),
            }),
        }),
    };
    fs::write(
        f.intent
            .output_dir
            .join("trusted-stop-fixture-execution.json"),
        serde_json::to_vec(&facts).unwrap(),
    )
    .unwrap();
    let cancel =
        f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(
        cancel.last().unwrap()["result"]["acknowledgement"]["acknowledged"],
        true,
        "{cancel:?}"
    );
    assert!(!f.intent.output_dir.join("report.json").exists());
    f.events(&[
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"clean"}),
    ]);
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "cancelled",
        "{frames:?}"
    );
    // The cleanup entry uses retained generation/labels even after the broker pin changes.
    let options = f.options.clone();
    fs::write(&options.instance_file, "local:///replacement\n").unwrap();
    fs::write(&f.config, serde_json::to_vec(&options).unwrap()).unwrap();
    let cleanup = f.call(
        json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}),
    );
    assert_eq!(
        cleanup.last().unwrap()["result"]["cleanup"]["stopped"],
        true,
        "{cleanup:?}"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn imported_module_tamper_refuses_before_dispatch_without_manifest_or_executable_change() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    let exe = fs::read(&f.request.runner.executable).unwrap();
    let manifest = fs::read(&f.request.runner.manifest).unwrap();
    let imported = f
        .request
        .runner
        .manifest
        .parent()
        .unwrap()
        .join("lib/tl3_harness/runtime.py");
    fs::set_permissions(&imported, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&imported, b"# changed trusted import\n").unwrap();
    fs::set_permissions(imported, fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(fs::read(&f.request.runner.executable).unwrap(), exe);
    assert_eq!(fs::read(&f.request.runner.manifest).unwrap(), manifest);
    assert_eq!(f.submit()["terminal"]["ending"], "refused");
    assert!(!f.calls().contains("job submit "));
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn sealed_runtime_entrypoints_and_arguments_cannot_be_overridden() {
    for changed in ["cleanup-argv", "container-python", "podman", "preflight"] {
        let mut f = Fixture::new();
        f.podman(ContainerMode::OwnScope, true);
        match changed {
            "cleanup-argv" => f
                .request
                .runtime
                .cleanup
                .as_mut()
                .unwrap()
                .arguments
                .push("--unbound".into()),
            "container-python" => {
                if let ExecutionRuntime::RootlessPodman { interpreter, .. } =
                    &mut f.request.runtime.runtime
                {
                    *interpreter = "/usr/bin/python3.14".into();
                }
            }
            "podman" => {
                if let ExecutionRuntime::RootlessPodman { executable, .. } =
                    &mut f.request.runtime.runtime
                {
                    *executable = "/usr/local/bin/podman".into();
                }
            }
            "preflight" => {
                let copy = f.root.path().join("unbound-preflight.json");
                fs::copy(&f.request.runtime.preflight.path, &copy).unwrap();
                f.request.runtime.preflight.path = copy;
            }
            _ => unreachable!(),
        }
        f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
        assert_eq!(f.submit()["terminal"]["ending"], "refused", "{changed}");
        assert!(!f.calls().contains("job submit "), "{changed}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn append_only_container_executions_require_every_independent_stop() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    f.retain_fixture_allocation();
    let first = f.container_ownership();
    let mut second = first.clone();
    second.execution = "second-execution".into();
    let mut launcher = second.clone();
    launcher.container = None;
    let frames = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let launch: ContainerLaunchIntent =
        serde_json::from_value(frames.last().unwrap()["result"]["launch"].clone()).unwrap();
    let container = second.container.as_mut().unwrap();
    container.id = "e".repeat(64);
    container.name = launch.name;
    container.labels = launch.labels;
    container
        .labels
        .insert("tl3.execution".into(), second.execution.clone());
    let scope = container.scope.as_mut().unwrap();
    scope.unit = format!("libpod-{}.scope", container.id);
    scope.invocation_id = "f".repeat(32);
    scope.cgroup = format!(
        "/user.slice/user-1000.slice/user@1000.service/app.slice/{}",
        scope.unit
    );
    for owned in [&first, &second] {
        let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    }
    let mut conflict = first.clone();
    conflict.launcher.start_ticks += 1;
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":conflict}));
    assert_eq!(frames.last().unwrap()["kind"], "error");
    let mut duplicate = first.clone();
    duplicate.execution = "duplicate".into();
    duplicate
        .container
        .as_mut()
        .unwrap()
        .labels
        .insert("tl3.execution".into(), duplicate.execution.clone());
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":duplicate}));
    assert_eq!(frames.last().unwrap()["kind"], "error");
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    let mut late = second.clone();
    late.execution = "late".into();
    late.container.as_mut().unwrap().id = "a".repeat(64);
    late.container
        .as_mut()
        .unwrap()
        .labels
        .insert("tl3.execution".into(), late.execution.clone());
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":late}));
    assert_eq!(frames.last().unwrap()["kind"], "error");
    let debts = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(
        debts
            .join(format!("{}.{}", f.intent.correlation, second.execution))
            .exists()
    );
    for (index, owned) in [&first, &second].into_iter().enumerate() {
        let container = owned.container.as_ref().unwrap();
        let scope = container.scope.as_ref().unwrap();
        let facts = CleanupEvidence {
            schema: "tl3.cleanup_evidence.v1".into(),
            pending_launch: None,
            ownership: owned.clone(),
            launcher: ProcessStop::Absent,
            process_group_probe_errno: Some(3),
            container: Some(ContainerStop {
                name: container.name.clone(),
                name_exists_exit: 1,
                sweep_exit: 0,
                remaining_ids: vec![],
                id: container.id.clone(),
                remove_exit: 0,
                exists_exit: 1,
                sweep_ids: vec![container.id.clone()],
                scope: Some(ScopeStop {
                    identity: scope.clone(),
                    observed_invocation_id: None,
                    cgroup_events: None,
                    cgroup_procs: String::new(),
                    active_state: "not-found".into(),
                }),
            }),
        };
        let reference = api::durable::blob(
            &f.intent.output_dir.join(format!("stop-{index}.json")),
            &serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
        let frames = f.call(json!({"op":"retain_cleanup","owner":f.intent.owner,"correlation":f.intent.correlation,"evidence":reference}));
        assert_eq!(
            frames.last().unwrap()["result"]["cleanup"]["stopped"],
            index == 1,
            "{frames:?}"
        );
        assert_eq!(
            frames.last().unwrap()["result"]["cleanup"]["accepted_execution"],
            owned.execution
        );
        if index == 0 {
            assert_eq!(
                frames.last().unwrap()["result"]["cleanup"]["pending_executions"],
                json!([second.execution])
            );
            assert!(
                debts
                    .join(format!("{}.{}", f.intent.correlation, second.execution))
                    .exists()
            );
            f.events(&[
                json!({"name":"exception","context":{"type":"cancel","severity":0}}),
                json!({"name":"clean"}),
            ]);
            assert_eq!(
                f.attach().last().unwrap()["result"]["observation"]["kind"],
                "unknown"
            );
        }
    }
    // Repeated cancel must not recreate debt for executions already proved stopped.
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(fs::read_dir(&debts).unwrap().count(), 0);
    f.events(&[
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"clean"}),
    ]);
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "cancelled",
        "{frames:?}"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn fresh_native_liveness_is_separate_from_historical_stream_and_terminal_wins() {
    let f = Fixture::new();
    f.submit();
    let jobs = |state| {
        fs::write(
            f.root.path().join("jobs.json"),
            json!({"name":f.intent.correlation,"jobid":"fABC","state":state}).to_string(),
        )
        .unwrap()
    };
    for (state, phase) in [
        ("SCHED", "queued"),
        ("RUN", "running"),
        ("CLEANUP", "finishing"),
    ] {
        jobs(state);
        let frames = f.call(
            json!({"op":"liveness","owner":f.intent.owner,"correlation":f.intent.correlation}),
        );
        assert_eq!(
            frames.last().unwrap()["result"]["liveness"]["phase"],
            phase,
            "{frames:?}"
        );
        assert_eq!(
            frames.last().unwrap()["result"]["liveness"]["authority"],
            "fresh_native"
        );
    }
    f.events(&completed(0));
    fs::write(f.root.path().join("mode"), "interrupted").unwrap();
    let streamed = f.attach();
    assert_eq!(streamed[0]["event"]["progress"]["authority"], "historical");
    assert_eq!(streamed[0]["event"]["progress"]["run"]["job"], "fABC");
    fs::write(f.root.path().join("mode"), "query-error").unwrap();
    let lost =
        f.call(json!({"op":"liveness","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(
        lost.last().unwrap()["result"]["liveness"]["phase"],
        "unknown"
    );
    assert_eq!(
        lost.last().unwrap()["result"]["liveness"]["authority"],
        "unknown"
    );
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    jobs("RUN");
    f.report(ReportOutcome::Passed, true);
    let before_terminal = f.state();
    let attached = f.attach();
    assert_eq!(
        attached.last().unwrap()["result"]["observation"]["kind"],
        "terminal",
        "{attached:?}"
    );
    let committed = f.state();
    let expected = &attached.last().unwrap()["result"]["observation"]["terminal"];
    let owner = Owner {
        owner: f.intent.owner.clone(),
        correlation: f.intent.correlation.clone(),
    };
    let journal = api::durable::Journal::new(f.options.journal_root.clone()).unwrap();
    fs::write(
        f.root.path().join("query-state-path"),
        f.options
            .journal_root
            .join(&f.intent.correlation)
            .join("state.json")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    for mode in ["terminal-query", "terminal-query-error"] {
        // Replay the pre-commit snapshot; native I/O publishes the verified terminal.
        // Report preparation and attach need not fit inside a native query's deadline.
        journal
            .transaction(&owner, |dir, _| api::durable::save(dir, &before_terminal))
            .unwrap();
        api::durable::atomic(
            &f.root.path().join("query-terminal-state.json"),
            &serde_json::to_vec(&committed).unwrap(),
        )
        .unwrap();
        fs::write(f.root.path().join("mode"), mode).unwrap();
        let calls = native_calls(&f.calls(), "jobs --json ");
        let frames = f.call(
            json!({"op":"liveness","owner":f.intent.owner,"correlation":f.intent.correlation}),
        );
        assert_eq!(native_calls(&f.calls(), "jobs --json "), calls + 1);
        let live = &frames.last().unwrap()["result"]["liveness"];
        assert_eq!(live["phase"], "terminal", "{mode}: {frames:?}");
        assert_eq!(
            live["authority"], "committed_terminal",
            "{mode}: {frames:?}"
        );
        assert_eq!(live["terminal"], *expected, "{mode}: {frames:?}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn prelaunch_crash_hang_and_cancel_recover_only_observed_named_container() {
    for bind_after_cancel in [false, true] {
        let mut f = Fixture::new();
        f.podman(ContainerMode::OwnScope, true);
        f.submit();
        f.retain_fixture_allocation();
        let observed = f.container_ownership();
        let mut launcher = observed.clone();
        launcher.container = None;
        let again = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
        assert_eq!(again.last().unwrap()["code"], "conflict");
        let none = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":launcher}));
        assert_eq!(none.last().unwrap()["kind"], "error");
        let absent = CleanupEvidence {
            schema: "tl3.cleanup_evidence.v1".into(),
            pending_launch: None,
            ownership: launcher.clone(),
            launcher: ProcessStop::Absent,
            process_group_probe_errno: Some(3),
            container: None,
        };
        let reference = api::durable::blob(
            &f.intent.output_dir.join("launcher-only.json"),
            &serde_json::to_vec(&absent).unwrap(),
        )
        .unwrap();
        let none = f.call(json!({"op":"retain_cleanup","owner":f.intent.owner,"correlation":f.intent.correlation,"evidence":reference}));
        assert_eq!(none.last().unwrap()["result"]["cleanup"]["stopped"], false);
        assert_eq!(
            none.last().unwrap()["result"]["cleanup"]["pending_executions"],
            json!([observed.execution])
        );
        f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
        f.events(&[
            json!({"name":"exception","context":{"type":"cancel","severity":0}}),
            json!({"name":"clean"}),
        ]);
        assert_eq!(
            f.attach().last().unwrap()["result"]["observation"]["kind"],
            "unknown"
        );
        let held = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
        assert_eq!(held.last().unwrap()["kind"], "error");
        if bind_after_cancel {
            let bound = f.call(json!({"op":"bind_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"observed":observed}));
            assert_eq!(
                bound.last().unwrap()["result"]["binding"]["payload_allowed"],
                false,
                "{bound:?}"
            );
        }
        let container = observed.container.as_ref().unwrap();
        let scope = container.scope.as_ref().unwrap();
        let mut facts = CleanupEvidence {
            schema: "tl3.cleanup_evidence.v1".into(),
            pending_launch: None,
            ownership: observed.clone(),
            launcher: ProcessStop::Absent,
            process_group_probe_errno: Some(3),
            container: Some(ContainerStop {
                name: container.name.clone(),
                name_exists_exit: 1,
                sweep_exit: 0,
                remaining_ids: vec![container.id.clone()],
                id: container.id.clone(),
                remove_exit: 0,
                exists_exit: 1,
                sweep_ids: vec![container.id.clone()],
                scope: Some(ScopeStop {
                    identity: scope.clone(),
                    observed_invocation_id: None,
                    cgroup_events: None,
                    cgroup_procs: String::new(),
                    active_state: "not-found".into(),
                }),
            }),
        };
        let path = f
            .intent
            .output_dir
            .join(format!("trusted-stop-{}.json", observed.execution));
        fs::write(&path, serde_json::to_vec(&facts).unwrap()).unwrap();
        let pending = f.call(json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_eq!(
            pending.last().unwrap()["result"]["cleanup"]["stopped"],
            false,
            "{pending:?}"
        );
        facts.container.as_mut().unwrap().remaining_ids.clear();
        fs::write(&path, serde_json::to_vec(&facts).unwrap()).unwrap();
        let stopped = f.call(json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_eq!(
            stopped.last().unwrap()["result"]["cleanup"]["stopped"],
            true,
            "{stopped:?}"
        );
        let terminal = f.attach();
        assert_eq!(
            terminal.last().unwrap()["result"]["observation"]["terminal"]["ending"],
            "cancelled",
            "{terminal:?}"
        );
        let reference: BlobRef = serde_json::from_value(
            terminal.last().unwrap()["result"]["observation"]["terminal"]["receipt"].clone(),
        )
        .unwrap();
        let receipt: Receipt = serde_json::from_slice(&fs::read(reference.path).unwrap()).unwrap();
        assert_eq!(receipt.container_launches.len(), 1);
        assert_eq!(receipt.runtime_ownerships, [observed]);
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn prelaunch_and_cancel_serialize_across_separate_cli_processes() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let launch = json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":f.ownership()});
    let cancel = json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation});
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
            .args(["--config", f.config.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut first = spawn();
    let mut second = spawn();
    writeln!(first.stdin.take().unwrap(), "{launch}").unwrap();
    writeln!(second.stdin.take().unwrap(), "{cancel}").unwrap();
    let launch_output = first.wait_with_output().unwrap();
    let cancel_output = second.wait_with_output().unwrap();
    assert!(cancel_output.status.success());
    let launch_frame: Value = serde_json::from_slice(&launch_output.stdout).unwrap();
    let record: api::durable::Record = serde_json::from_slice(
        &fs::read(
            f.options
                .journal_root
                .join(&f.intent.correlation)
                .join("state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(record.cancel_requested);
    assert!(record.runtime_ownerships.is_empty());
    if launch_output.status.success() {
        assert_eq!(launch_frame["kind"], "result");
        assert_eq!(record.container_launches.len(), 1);
    } else {
        assert_eq!(launch_frame["code"], "acceptance_unknown", "{launch_frame}");
        assert_eq!(
            launch_frame["message"],
            "container launch is held after cancellation or terminal"
        );
        assert!(record.container_launches.is_empty());
    }
    for intent in record.container_launches.values() {
        assert_eq!(intent.launcher, f.ownership());
        assert!(
            f.options
                .journal_root
                .join("cleanup-debt")
                .join(&f.request.runtime.manifest_digest)
                .join(format!(
                    "{}.{}",
                    f.intent.correlation, intent.launcher.execution
                ))
                .exists()
        );
    }
    let held = f.call(launch);
    assert_eq!(held.last().unwrap()["kind"], "error");
    let cleanup = f.call(
        json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}),
    );
    assert_eq!(
        cleanup.last().unwrap()["result"]["cleanup"]["stopped"],
        false
    );
}

fn worker_fixture(suite: Suite) -> Fixture {
    let mut f = Fixture::new();
    f.request.suite = suite;
    let frames = f.call(json!({"op":"prepare_intent","owner":f.intent.owner,"correlation":f.intent.correlation,"request":f.request}));
    f.intent = serde_json::from_value(frames.last().unwrap()["result"]["intent"].clone()).unwrap();
    f.submit();
    f.report(ReportOutcome::WorkerPayloadCompleted, true);
    f
}

fn collect_worker(f: &Fixture, raw: &[u8]) -> Vec<Value> {
    let stdout = api::durable::blob(&f.intent.output_dir.join("raw-worker-stdout"), raw).unwrap();
    f.call(json!({"op":"collect_worker","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{
        "run":f.ownership().run,"stdout":stdout,"runtime_binding":null,"cleanup":null}}))
}

fn test_summary() -> Value {
    json!({"status":"completed","tests":[{"test":"test_subject","outcome":"failed"}],
        "totals":{"passed":0,"failed":1,"error":0,"skipped":0},"note":"semantic judging remains with worker"})
}

#[test]
fn protocol_bounds_are_config_free_generated_scalars() {
    for suite in [
        Suite::WorkerTests {
            input_digest: "a".repeat(64),
        },
        Suite::Generator {
            input_digest: "b".repeat(64),
        },
        Suite::DaytimeChecks {
            base: "a".repeat(40),
            head: "b".repeat(40),
            reviewed: None,
            plan_digest: "c".repeat(64),
        },
    ] {
        let frames = cli(&json!({"op":"output_bounds","suite":suite}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        let bounds: OutputBounds =
            serde_json::from_value(frames.last().unwrap()["result"]["bounds"].clone()).unwrap();
        assert_eq!(bounds, output_bounds(&suite));
        assert_eq!(
            bounds.cli_result_bytes,
            bounds.payload_bytes + bounds.cli_envelope_bytes
        );
        assert!(bounds.stderr_bytes > 0);
        assert!(bounds.stdout_bytes >= bounds.payload_bytes);
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn worker_frames_and_verified_terminal_preserve_symptom_and_owner_identity() {
    let f = worker_fixture(Suite::WorkerTests {
        input_digest: "a".repeat(64),
    });
    let records = json!([{"event":"result","test":"test_subject","outcome":"error"}]);
    let receipt = json!({"run":1,"sha256":digest(records.to_string().as_bytes()),"records":records,"outcome":"symptom","target_hits":{}});
    let mut entry = receipt.clone();
    entry.as_object_mut().unwrap().remove("records");
    let mut summary = test_summary();
    summary["declared_symptom"] = json!({"repeat":1,"node":"test_subject","exception":"builtins.ValueError","phase":"call","parallel":0});
    summary["symptom_runs"] = json!([entry]);
    let raw = format!("diagnostic\nTL3_SYMPTOM_RUN {receipt}\nTL3_TEST_SUMMARY {summary}\n");
    let frames = collect_worker(&f, raw.as_bytes());
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let expected = frames.last().unwrap()["result"]["collection"]["payload"].clone();
    let retained_report = f.intent.output_dir.join("report.json");
    let report: Value = serde_json::from_slice(&fs::read(&retained_report).unwrap()).unwrap();
    f.events(&completed(0));
    for field in [
        "request_digest",
        "tree",
        "runner_digest",
        "runtime_digest",
        "suite",
        "run",
    ] {
        let mut wrong = report.clone();
        match field {
            "tree" => wrong["tree"]["head"] = json!("d".repeat(40)),
            "suite" => wrong["suite"]["input_digest"] = json!("d".repeat(64)),
            "run" => wrong["run"]["job"] = json!("fABD"),
            _ => wrong[field] = json!("d".repeat(64)),
        }
        fs::write(&retained_report, wrong.to_string()).unwrap();
        let attach = f.attach();
        assert_eq!(
            attach.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{field}: {attach:?}"
        );
    }
    fs::write(&retained_report, report.to_string()).unwrap();
    let attach = f.attach();
    let terminal = attach.last().unwrap()["result"]["observation"]["terminal"].clone();
    // A failed test is a valid transport payload, never native rc-only semantic judgment.
    assert_eq!(terminal["ending"], "passed", "{attach:?}");
    let result = f.call(json!({"op":"worker_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(
        result.last().unwrap()["result"]["payload"],
        expected,
        "{result:?}"
    );
    let reference: BlobRef = serde_json::from_value(expected["symptom_runs"][0].clone()).unwrap();
    assert_eq!(
        fs::read(&reference.path).unwrap(),
        receipt.to_string().as_bytes()
    );
    let other = f.call(json!({"op":"worker_payload","owner":"other-owner","correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(other.last().unwrap()["kind"], "error");
    fs::write(reference.path, b"{}").unwrap();
    let corrupt = f.call(json!({"op":"worker_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(corrupt.last().unwrap()["kind"], "error");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn worker_collector_rejects_missing_duplicate_malformed_and_misbound_protocols() {
    let f = worker_fixture(Suite::WorkerTests {
        input_digest: "a".repeat(64),
    });
    let valid = test_summary();
    for raw in [
        "no report\n".to_owned(),
        "TL3_TEST_SUMMARY {\n".into(),
        "TL3_TEST_SUMMARY {}\n".into(),
        format!("TL3_TEST_SUMMARY {valid}\nTL3_TEST_SUMMARY {valid}\n"),
        format!("TL3_SYMPTOM_RUN {{}}\nTL3_TEST_SUMMARY {valid}\n"),
        format!("TL3_GENERATOR_ARTIFACT {{}}\nTL3_TEST_SUMMARY {valid}\n"),
    ] {
        let result = collect_worker(&f, raw.as_bytes());
        assert_eq!(result.last().unwrap()["kind"], "error", "{result:?}");
    }
    let mut missing = valid.clone();
    missing["declared_symptom"] = json!({"repeat":1});
    missing["symptom_runs"] = json!([{"run":1}]);
    assert_eq!(
        collect_worker(&f, format!("TL3_TEST_SUMMARY {missing}\n").as_bytes())
            .last()
            .unwrap()["kind"],
        "error"
    );
    let stdout = api::durable::blob(
        &f.intent.output_dir.join("raw-worker-stdout"),
        format!("TL3_TEST_SUMMARY {valid}\n").as_bytes(),
    )
    .unwrap();
    let mut run = f.ownership().run;
    run.job = "fABD".into();
    let wrong = f.call(json!({"op":"collect_worker","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{"run":run,"stdout":stdout,"cleanup":null,"runtime_binding":null}}));
    assert_eq!(wrong.last().unwrap()["kind"], "error");
    let stderr = api::durable::blob(
        &f.intent.output_dir.join("raw-worker-stderr"),
        &vec![b'x'; output_bounds(&f.request.suite).stderr_bytes + 1],
    )
    .unwrap();
    let wrong = f.call(json!({"op":"collect_worker","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{"run":f.ownership().run,"stdout":stdout,"stderr":stderr,"cleanup":null,"runtime_binding":null}}));
    assert_eq!(wrong.last().unwrap()["kind"], "error");
    assert_eq!(
        collect_worker(&f, format!("TL3_TEST_SUMMARY {valid}\n").as_bytes())
            .last()
            .unwrap()["kind"],
        "result"
    );
}

fn generator_artifact(outputs: Value) -> Value {
    json!({"status":"completed","exit_code":0,"message":"Generator completed.","command":"generate","argv":["generate"],
        "input_manifest":"a".repeat(64),"output_manifest":"b".repeat(64),"outputs":outputs})
}

fn generator_output(name: &str, bytes: &[u8]) -> Value {
    use base64::{Engine, engine::general_purpose::STANDARD};
    json!({"path":name,"entry":{"bytes":bytes.len(),"sha256":digest(bytes),"mode":420},"data":STANDARD.encode(bytes)})
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generator_collector_preserves_large_artifact_and_refuses_limits_and_corruption() {
    let f = worker_fixture(Suite::Generator {
        input_digest: "a".repeat(64),
    });
    let small = generator_output("generated/a.txt", b"generated text\n");
    let valid = generator_artifact(json!([small]));
    let mut wrong = valid.clone();
    wrong["outputs"][0]["entry"]["sha256"] = json!("c".repeat(64));
    let mut traversal = valid.clone();
    traversal["outputs"][0]["path"] = json!("../escape");
    let mut bad_base64 = valid.clone();
    bad_base64["outputs"][0]["data"] = json!("%%%invalid");
    let mut duplicate = valid.clone();
    duplicate["outputs"]
        .as_array_mut()
        .unwrap()
        .push(small.clone());
    for raw in [
        "missing\n".to_owned(),
        "TL3_GENERATOR_ARTIFACT {\n".into(),
        "TL3_GENERATOR_ARTIFACT {}\n".into(),
        format!("TL3_GENERATOR_ARTIFACT {valid}\nTL3_GENERATOR_ARTIFACT {valid}\n"),
        format!("TL3_GENERATOR_ARTIFACT {wrong}\n"),
        format!("TL3_GENERATOR_ARTIFACT {traversal}\n"),
        format!("TL3_GENERATOR_ARTIFACT {bad_base64}\n"),
        format!("TL3_GENERATOR_ARTIFACT {duplicate}\n"),
    ] {
        assert_eq!(
            collect_worker(&f, raw.as_bytes()).last().unwrap()["kind"],
            "error"
        );
    }
    let bounds = output_bounds(&f.request.suite);
    for outputs in [
        json!([generator_output(
            "large.txt",
            &vec![b'x'; bounds.artifact_file_bytes + 1]
        )]),
        Value::Array(
            (0..bounds.artifact_files + 1)
                .map(|n| generator_output(&format!("file{n}"), b"x"))
                .collect(),
        ),
        Value::Array(
            (0..3)
                .map(|n| {
                    generator_output(
                        &format!("file{n}"),
                        &vec![b'x'; bounds.decoded_artifact_bytes / 3 + 1],
                    )
                })
                .collect(),
        ),
    ] {
        let artifact = generator_artifact(outputs);
        assert_eq!(
            collect_worker(
                &f,
                format!("TL3_GENERATOR_ARTIFACT {artifact}\n").as_bytes()
            )
            .last()
            .unwrap()["kind"],
            "error"
        );
    }
    let mut oversized = format!("TL3_GENERATOR_ARTIFACT {valid}").into_bytes();
    oversized.resize(
        b"TL3_GENERATOR_ARTIFACT ".len() + bounds.payload_bytes + 1,
        b' ',
    );
    oversized.push(b'\n');
    assert_eq!(
        collect_worker(&f, &oversized).last().unwrap()["kind"],
        "error"
    );
    let failed = json!({"status":"failed","exit_code":null,"message":"Generator action infrastructure failed."});
    assert_eq!(
        collect_worker(&f, format!("TL3_GENERATOR_ARTIFACT {failed}\n").as_bytes())
            .last()
            .unwrap()["kind"],
        "result"
    );
    let artifact = generator_artifact(json!([generator_output(
        "large.txt",
        &vec![b'x'; 7 * 1024 * 1024]
    )]));
    let bytes = format!("TL3_GENERATOR_ARTIFACT {artifact}\n");
    assert!(bytes.len() > api::durable::MAX_BYTES);
    let result = collect_worker(&f, bytes.as_bytes());
    assert_eq!(result.last().unwrap()["kind"], "result", "{result:?}");
    let expected = result.last().unwrap()["result"]["collection"]["payload"].clone();
    f.events(&completed(0));
    let attach = f.attach();
    let terminal = attach.last().unwrap()["result"]["observation"]["terminal"].clone();
    assert_eq!(terminal["ending"], "passed", "{attach:?}");
    let result = f.call(json!({"op":"worker_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(
        result.last().unwrap()["result"]["payload"],
        expected,
        "{result:?}"
    );
    let reference: BlobRef = serde_json::from_value(expected["artifact"].clone()).unwrap();
    assert_eq!(
        fs::read(reference.path).unwrap(),
        artifact.to_string().as_bytes()
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn daytime_collector_preserves_near_eight_mib_complete_refusal_and_refuses_overflow() {
    let f = worker_fixture(Suite::DaytimeChecks {
        base: "b".repeat(40),
        head: "c".repeat(40),
        reviewed: None,
        plan_digest: "a".repeat(64),
    });
    let bounds = output_bounds(&f.request.suite);
    let mut report = json!({"schema":"tl3.checks.v3","effect_key":"fixture","base":"b".repeat(40),"head":"c".repeat(40),"reviewed":null,"results":[{
        "name":"complete refusal","kind":"demonstrating","verdict":"infrastructure-failure","custody":"in-process","tests":[]}]});
    let tests = report["results"][0]["tests"].as_array_mut().unwrap();
    let refusal = "complete proof refusal ".repeat(2800);
    for index in 0..129 {
        tests.push(json!({"test":format!("test_{index}"),"base":"uncollected","head":"uncollected","pre_existing":false,"passed":false,"message":"summary","proof_refusal":refusal}));
    }
    let bytes = report.to_string();
    assert!(
        bytes.len() > bounds.payload_bytes - 128 * 1024 && bytes.len() < bounds.payload_bytes,
        "{}",
        bytes.len()
    );
    let stdout = api::durable::blob(
        &f.intent.output_dir.join("near-budget-stdout"),
        format!("TL3_CHECKS {bytes}\n").as_bytes(),
    )
    .unwrap();
    // Stderr is additional, not deducted from the legal stdout frame budget.
    let stderr = api::durable::blob(
        &f.intent.output_dir.join("near-budget-stderr"),
        &vec![b'x'; bounds.stderr_bytes],
    )
    .unwrap();
    let result = f.call(json!({"op":"collect_runner","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{"run":f.ownership().run,"stdout":stdout,"stderr":stderr,"runtime_binding":null,"cleanup":null}}));
    assert_eq!(result.last().unwrap()["kind"], "result", "{result:?}");
    let payload: BlobRef =
        serde_json::from_value(result.last().unwrap()["result"]["payload"].clone()).unwrap();
    assert_eq!(fs::read(&payload.path).unwrap(), bytes.as_bytes());
    let mut over = bytes.into_bytes();
    over.extend(std::iter::repeat_n(
        b' ',
        bounds.payload_bytes + 1 - over.len(),
    ));
    let mut raw = b"TL3_CHECKS ".to_vec();
    raw.extend(over);
    raw.push(b'\n');
    let stdout = api::durable::blob(&f.intent.output_dir.join("overflow-stdout"), &raw).unwrap();
    let result = f.call(json!({"op":"collect_runner","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{"run":f.ownership().run,"stdout":stdout,"runtime_binding":null,"cleanup":null}}));
    assert_eq!(result.last().unwrap()["kind"], "error", "{result:?}");
    f.events(&completed(0));
    let attach = f.attach();
    let terminal = attach.last().unwrap()["result"]["observation"]["terminal"].clone();
    assert_eq!(terminal["ending"], "passed", "{attach:?}");
    let result = f.call(json!({"op":"daytime_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(
        result.last().unwrap()["result"]["report"],
        report,
        "retained complete refusal changed"
    );
}

#[test]
fn cleanup_limits_are_config_free_and_bind_both_transport_operations() {
    let frames = cli(&json!({"op":"cleanup_limits"}));
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["sequence"], 0);
    assert_eq!(frames[0]["result"]["op"], "cleanup_limits");
    let limits: CleanupLimits =
        serde_json::from_value(frames[0]["result"]["limits"].clone()).unwrap();
    assert_eq!(limits, cleanup_limits());
    assert!(limits.helper_budget_seconds < limits.native_timeout_seconds);
    assert!(limits.native_timeout_seconds < limits.idle_timeout_seconds);
    assert_eq!(limits.max_progress_frames, 1024);
    assert!(limits.max_response_bytes >= limits.max_progress_frames * limits.max_frame_bytes);
}

impl Fixture {
    fn cleanup_execution(&self, execution: &str, digit: char) -> RuntimeOwnership {
        let mut owned = self.ownership();
        owned.execution = execution.into();
        let frames = self.call(json!({"op":"begin_container_launch","owner":self.intent.owner,"correlation":self.intent.correlation,"launcher":owned}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        let launch: ContainerLaunchIntent =
            serde_json::from_value(frames.last().unwrap()["result"]["launch"].clone()).unwrap();
        let id = digit.to_string().repeat(64);
        let unit = format!("libpod-{id}.scope");
        owned.container = Some(ContainerOwnership {
            name: launch.name,
            labels: launch.labels,
            id,
            mode: ContainerMode::OwnScope,
            scope: Some(ScopeIdentity {
                cgroup: format!("/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}"),
                unit,
                invocation_id: digit.to_string().repeat(32),
            }),
        });
        let frames = self.call(json!({"op":"retain_runtime","owner":self.intent.owner,"correlation":self.intent.correlation,"ownership":owned}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        self.cleanup_facts(&owned, true);
        owned
    }

    fn cleanup_facts(&self, owned: &RuntimeOwnership, stopped: bool) {
        let container = owned.container.as_ref().unwrap();
        let facts = CleanupEvidence {
            schema: "tl3.cleanup_evidence.v1".into(),
            pending_launch: None,
            ownership: owned.clone(),
            launcher: ProcessStop::Absent,
            process_group_probe_errno: Some(3),
            container: Some(ContainerStop {
                name: container.name.clone(),
                name_exists_exit: if stopped { 1 } else { 0 },
                sweep_exit: 0,
                remaining_ids: if stopped {
                    vec![]
                } else {
                    vec![container.id.clone()]
                },
                id: container.id.clone(),
                remove_exit: 0,
                exists_exit: if stopped { 1 } else { 0 },
                sweep_ids: vec![container.id.clone()],
                scope: Some(ScopeStop {
                    identity: container.scope.as_ref().unwrap().clone(),
                    observed_invocation_id: None,
                    cgroup_events: None,
                    cgroup_procs: String::new(),
                    active_state: "not-found".into(),
                }),
            }),
        };
        fs::write(
            self.intent
                .output_dir
                .join(format!("trusted-stop-{}.json", owned.execution)),
            serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
    }

    fn cleanup_control(&self, execution: &str, delay: u64) {
        fs::write(
            self.intent
                .output_dir
                .join(format!("helper-control-{execution}.json")),
            json!({"delay":delay}).to_string(),
        )
        .unwrap();
    }

    fn state(&self) -> api::durable::Record {
        serde_json::from_slice(
            &fs::read(
                self.options
                    .journal_root
                    .join(&self.intent.correlation)
                    .join("state.json"),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn spawn_operation(
        &self,
        op: &str,
        startup_delay_seconds: u64,
    ) -> (
        std::process::Child,
        std::sync::mpsc::Receiver<Value>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{BufRead, BufReader};
        let mut command = if startup_delay_seconds == 0 {
            Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        } else {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    r#"/bin/sleep "$1"; shift; exec "$@""#,
                    "cleanup-startup-delay",
                ])
                .arg(startup_delay_seconds.to_string())
                .arg(env!("CARGO_BIN_EXE_tl3-run-backend"));
            command
        };
        let mut child = command
            .args(["--config", self.config.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(
            child.stdin.take().unwrap(),
            "{}",
            json!({"op":op,"owner":self.intent.owner,"correlation":self.intent.correlation})
        )
        .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let frame = serde_json::from_str(&line.unwrap()).unwrap();
                if send.send(frame).is_err() {
                    break;
                }
            }
        });
        (child, receive, reader)
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cleanup_transport_progress_spans_old_total_deadline_for_cleanup_and_cancel() {
    for op in ["cleanup_runtime", "cancel"] {
        let mut f = Fixture::new();
        f.podman(ContainerMode::OwnScope, true);
        f.submit();
        let executions = [
            f.cleanup_execution("first", 'd'),
            f.cleanup_execution("second", 'e'),
        ];
        for owned in &executions {
            f.cleanup_control(&owned.execution, 16);
        }
        let start = Instant::now();
        let (mut child, receive, reader) = f.spawn_operation(op, 0);
        let limits = cleanup_limits();
        let mut frames = vec![];
        loop {
            let frame = receive
                .recv_timeout(Duration::from_secs(limits.idle_timeout_seconds))
                .expect("bounded transport idle timeout");
            assert_eq!(frame["sequence"], frames.len() as u64);
            assert!(serde_json::to_vec(&frame).unwrap().len() <= limits.max_frame_bytes);
            let done = frame["kind"] == "result" || frame["kind"] == "error";
            if !done {
                assert_eq!(frame["kind"], "cleanup_progress", "{frame}");
                let progress: CleanupProgress =
                    serde_json::from_value(frame["progress"].clone()).unwrap();
                let owned = &executions[frames.len()];
                assert_eq!(progress.owner, owned.owner);
                assert_eq!(progress.run, owned.run);
                assert_eq!(progress.request_digest, owned.request_digest);
                assert_eq!(progress.execution, owned.execution);
                assert_eq!(progress.helper_budget_seconds, limits.helper_budget_seconds);
                assert_eq!(progress.idle_timeout_seconds, limits.idle_timeout_seconds);
                assert!(
                    f.state().terminal.is_none(),
                    "progress cannot complete the run"
                );
                assert_eq!(f.state().cancel_requested, op == "cancel");
            }
            frames.push(frame);
            if done {
                break;
            }
        }
        assert!(child.wait().unwrap().success(), "{frames:?}");
        reader.join().unwrap();
        assert!(receive.try_recv().is_err(), "only one final frame");
        assert_eq!(frames.len(), executions.len() + 1, "{frames:?}");
        assert!(start.elapsed() > Duration::from_secs(limits.native_timeout_seconds));
        assert_eq!(frames.last().unwrap()["result"]["op"], op);
        if op == "cleanup_runtime" {
            assert_eq!(frames.last().unwrap()["result"]["cleanup"]["stopped"], true);
        }
        for owned in &executions {
            let input: CleanupInput = serde_json::from_slice(
                &fs::read(
                    f.intent
                        .output_dir
                        .join(format!("helper-called-{}", owned.execution)),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(input.budget_seconds, limits.helper_budget_seconds);
            assert_eq!(input.ownership, *owned);
        }
        assert!(
            f.state().terminal.is_none(),
            "cleanup/cancel acknowledgement is not terminal"
        );
        assert_eq!(f.state().binding.unwrap().id, executions[0].run);
        assert_eq!(
            f.calls()
                .lines()
                .filter(|line| line.starts_with("cancel "))
                .count(),
            usize::from(op == "cancel")
        );
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cleanup_transport_silence_is_bounded_and_incomplete_evidence_preserves_debt() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("silent", 'd');
    f.cleanup_control(
        &owned.execution,
        cleanup_limits().native_timeout_seconds + 60,
    );
    let limits = cleanup_limits();
    let startup_delay = limits.idle_timeout_seconds - limits.native_timeout_seconds + 1;
    let start = Instant::now();
    let (mut child, receive, reader) = f.spawn_operation("cleanup_runtime", startup_delay);
    let progress = recv_within(
        "silent cleanup initial progress",
        &receive,
        transport_idle_deadline(),
    );
    assert_eq!(progress["kind"], "cleanup_progress", "{progress:?}");
    let initial_elapsed = start.elapsed();
    let silence_start = Instant::now();
    let result = recv_within(
        "silent cleanup final result",
        &receive,
        transport_idle_deadline(),
    );
    let silence_elapsed = silence_start.elapsed();
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    assert!(receive.try_recv().is_err(), "only one final frame");
    let frames = [progress, result];
    println!(
        "{}",
        json!({"cleanup_transport_timing": {
            "initial_seconds": initial_elapsed.as_secs_f64(),
            "silence_seconds": silence_elapsed.as_secs_f64(),
            "total_seconds": start.elapsed().as_secs_f64(),
            "idle_limit_seconds": limits.idle_timeout_seconds,
            "startup_delay_seconds": startup_delay
        }})
    );
    // Progress renews the idle allowance; total duration may exceed one allowance.
    assert!(
        start.elapsed() > transport_idle_deadline(),
        "delayed startup must exercise the difference between idle and total duration"
    );
    assert_eq!(frames[1]["result"]["cleanup"]["stopped"], false);
    assert!(f.state().cleanup_evidence.is_empty());
    f.cleanup_control(&owned.execution, 0);
    f.cleanup_facts(&owned, false);
    let frames = f.call(
        json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}),
    );
    assert_eq!(
        frames.last().unwrap()["result"]["cleanup"]["stopped"],
        false
    );
    assert!(f.intent.output_dir.read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("cleanup-")
    }));
    let debts = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(
        debts
            .join(format!("{}.{}", f.intent.correlation, owned.execution))
            .exists()
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cleanup_transport_callback_failure_starts_no_helper_and_cancel_intent_survives() {
    use tl3_controller::adapters::run_backend::flux::Flux;
    for cancel in [false, true] {
        let mut f = Fixture::new();
        f.podman(ContainerMode::OwnScope, true);
        f.submit();
        let owned = f.cleanup_execution("callback", 'd');
        let backend = Flux::new(f.options.clone()).unwrap();
        let mut broken =
            |_: &CleanupProgress| Err(BackendError::unknown("fixture transport disconnected"));
        let result = if cancel {
            backend
                .cancel_with_cleanup_progress(&owned.owner, &mut broken)
                .map(|_| ())
        } else {
            backend
                .cleanup_runtime_with_progress(&owned.owner, &mut broken)
                .map(|_| ())
        };
        assert!(result.is_err());
        assert!(!f.intent.output_dir.join("helper-called-callback").exists());
        assert_eq!(f.state().cancel_requested, cancel);
        assert!(f.state().cleanup_evidence.is_empty());
        assert!(f.state().terminal.is_none());
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cleanup_transport_separate_cancel_late_writes_preserve_binding_debt_and_terminal() {
    use std::time::Duration;
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    f.retain_fixture_allocation();
    let first = f.cleanup_execution("first", 'd');
    let second = f.cleanup_execution("second", 'e');
    f.cleanup_facts(&second, false);
    f.cleanup_control(&first.execution, 2);
    let (mut child, receive, reader) = f.spawn_operation("cleanup_runtime", 0);
    let progress = recv_within(
        "first cleanup progress frame",
        &receive,
        Duration::from_secs(5),
    );
    assert_eq!(progress["progress"]["execution"], "first");
    // Separate cancel fsyncs intent while the existing cleanup attempt owns the helper.
    let cancel =
        f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(cancel.last().unwrap()["result"]["op"], "cancel");
    assert!(f.state().cancel_requested);
    let mut launcher = f.ownership();
    launcher.execution = "late".into();
    let late = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
    assert_eq!(late.last().unwrap()["kind"], "error");
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    let frames: Vec<_> = receive.try_iter().collect();
    assert_eq!(
        frames.last().unwrap()["result"]["cleanup"]["stopped"],
        false
    );
    let state = f.state();
    assert_eq!(state.binding.as_ref().unwrap().id, first.run);
    assert_eq!(state.runtime_ownerships.len(), 2);
    assert!(state.cancel_requested);
    assert!(state.terminal.is_none());
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest)
        .join(format!("{}.{}", f.intent.correlation, second.execution));
    assert!(
        debt.exists(),
        "first cleanup cannot clear the second execution's debt"
    );
    f.cleanup_facts(&second, true);
    let due = maintenance_due(&f);
    let backend = maintenance_backend(&f);
    backend.cleanup_maintenance(due).unwrap();
    assert!(
        !debt.exists(),
        "cancel during in-flight cleanup guarantees a maintenance attempt after it finishes"
    );
    assert_eq!(
        fs::read_to_string(f.intent.output_dir.join("helper-attempts-second"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    f.events(&[
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"clean"}),
    ]);
    let terminal = f.attach().last().unwrap()["result"]["observation"]["terminal"].clone();
    assert_eq!(terminal["ending"], "cancelled");
    f.call(
        json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}),
    );
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(
        serde_json::to_value(f.state().terminal.unwrap()).unwrap(),
        terminal
    );
    assert!(f.state().cancel_requested);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn undispatched_cancel_commits_without_runtime_debt_or_native_effect() {
    let f = Fixture::new();
    fs::write(f.root.path().join("jobspec.py"), "raise SystemExit(1)\n").unwrap();
    assert_eq!(f.submit()["kind"], "unknown");
    assert!(!f.state().dispatched);
    let legacy_debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    fs::create_dir_all(&legacy_debt).unwrap();
    fs::write(
        legacy_debt.join(format!("{}.unregistered", f.intent.correlation)),
        serde_json::to_vec(&f.intent).unwrap(),
    )
    .unwrap();
    let frames =
        f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(
        frames.last().unwrap()["result"]["acknowledgement"]["terminal"]["ending"],
        "cancelled",
        "{frames:?}"
    );
    assert_eq!(f.state().terminal.unwrap().ending, Ending::Cancelled);
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(!debt.exists() || fs::read_dir(debt).unwrap().next().is_none());
    assert!(
        !fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .any(|line| line.starts_with("cancel "))
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn pending_injob_launch_requires_exact_stopped_launcher_and_empty_name_and_label_sweep() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    assert_eq!(f.submit()["kind"], "accepted");
    let launcher = f.ownership();
    let frames = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
    let launch = frames.last().unwrap()["result"]["launch"].clone();
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let facts = json!({"schema":"tl3.cleanup_evidence.v1","ownership":launcher,"launcher":{"kind":"absent"},
        "process_group_probe_errno":3,"container":null,
        "pending_launch":{"intent":launch,"name_exists_exit":1,"sweep_exit":0,"remaining_ids":[],"started_monotonic_ns":1,"finished_monotonic_ns":2}});
    for defect in [
        "name",
        "sweep",
        "remaining",
        "launcher",
        "group",
        "intent",
        "mode",
        "missing",
        "valid",
    ] {
        let mut evidence = facts.clone();
        match defect {
            "name" => evidence["pending_launch"]["name_exists_exit"] = json!(125),
            "sweep" => evidence["pending_launch"]["sweep_exit"] = json!(125),
            "remaining" => evidence["pending_launch"]["remaining_ids"] = json!(["c".repeat(64)]),
            "launcher" => evidence["launcher"] = json!({"kind":"unknown"}),
            "group" => evidence["process_group_probe_errno"] = Value::Null,
            "intent" => evidence["pending_launch"]["intent"]["name"] = json!("another-name"),
            "mode" => evidence["pending_launch"]["intent"]["mode"] = json!("own_scope"),
            "missing" => {
                evidence.as_object_mut().unwrap().remove("pending_launch");
            }
            _ => {}
        }
        let reference = api::durable::blob(
            &f.intent.output_dir.join(format!("pending-{defect}.json")),
            &serde_json::to_vec(&evidence).unwrap(),
        )
        .unwrap();
        let frames = f.call(json!({"op":"retain_cleanup","owner":f.intent.owner,"correlation":f.intent.correlation,"evidence":reference}));
        if defect == "valid" {
            assert_eq!(
                frames.last().unwrap()["result"]["cleanup"]["stopped"],
                true,
                "{frames:?}"
            );
        } else {
            assert_ne!(
                frames.last().unwrap()["result"]["cleanup"]["stopped"],
                true,
                "{defect}: {frames:?}"
            );
        }
    }
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(fs::read_dir(debt).unwrap().next().is_some());
    let mut late = launcher.clone();
    late.container = Some(ContainerOwnership {
        name: launch["name"].as_str().unwrap().into(),
        id: "c".repeat(64),
        mode: ContainerMode::InJob,
        labels: serde_json::from_value(launch["labels"].clone()).unwrap(),
        scope: None,
    });
    let frames = f.call(json!({"op":"bind_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"observed":late}));
    assert_eq!(
        frames.last().unwrap()["kind"],
        "error",
        "stopped pending intent must not acquire a late binding"
    );
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    f.events(&[
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"clean"}),
    ]);
    let terminal = f.attach();
    assert_eq!(
        terminal.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{terminal:?}"
    );
}

fn failed_pending_injob_launch(late_cancel: bool) {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    assert_eq!(f.submit()["kind"], "accepted");
    let launcher = f.ownership();
    let frames = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let launch = frames.last().unwrap()["result"]["launch"].clone();
    if late_cancel {
        f.events(&completed(256));
        f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    }
    let evidence = json!({"schema":"tl3.cleanup_evidence.v1","ownership":launcher,"launcher":{"kind":"absent"},
        "process_group_probe_errno":3,"container":null,
        "pending_launch":{"intent":launch,"name_exists_exit":1,"sweep_exit":0,"remaining_ids":[],"started_monotonic_ns":1,"finished_monotonic_ns":2}});
    let reference = api::durable::blob(
        &f.intent.output_dir.join("failed-pending.json"),
        &serde_json::to_vec(&evidence).unwrap(),
    )
    .unwrap();
    let frames = f.call(json!({"op":"retain_cleanup","owner":f.intent.owner,"correlation":f.intent.correlation,"evidence":reference}));
    assert_eq!(
        frames.last().unwrap()["result"]["cleanup"]["stopped"],
        true,
        "{frames:?}"
    );
    // A cancel after native Clean does not add an exception to the completed job log.
    f.events(&completed(256));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{frames:?}"
    );
    let state = f.state();
    assert_eq!(state.container_launches.len(), 1);
    assert!(state.runtime_ownerships.is_empty());
    assert_eq!(state.cancel_requested, late_cancel);
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    assert!(fs::read_dir(debt).unwrap().next().is_some());
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn failed_pending_injob_launch_finishes_without_cancel() {
    failed_pending_injob_launch(false);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn failed_pending_injob_launch_preserves_failure_after_late_cancel() {
    failed_pending_injob_launch(true);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn injob_no_launch_needs_native_clean_and_does_not_leave_unregistered_debt() {
    for cancelled in [false, true] {
        let mut f = Fixture::new();
        f.podman(ContainerMode::InJob, true);
        assert_eq!(f.submit()["kind"], "accepted");
        f.retain_fixture_allocation();
        if cancelled {
            f.call(
                json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}),
            );
        }
        let before = f.call(json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_eq!(
            before.last().unwrap()["result"]["cleanup"]["stopped"],
            false
        );
        let events = if cancelled {
            vec![
                json!({"name":"exception","context":{"type":"cancel","severity":0}}),
                json!({"name":"clean"}),
            ]
        } else {
            completed(256)
        };
        f.events(&events);
        let terminal = f.attach();
        assert_eq!(
            terminal.last().unwrap()["result"]["observation"]["terminal"]["ending"],
            if cancelled { "cancelled" } else { "infra-lost" },
            "{terminal:?}"
        );
        let debt = f
            .options
            .journal_root
            .join("cleanup-debt")
            .join(&f.request.runtime.manifest_digest);
        assert!(!debt.exists() || fs::read_dir(debt).unwrap().next().is_none());
    }
}

// FX-NO-GRANT-SCRATCH: a dispatched InJob job whose runner never validated an
// allocation has no grant and no execution. After native clean, the pinned
// helper proves the exact retained leaf absent, reclaiming an owned tree through
// the existing guarded flow, or the job stays held with its debt.
const SCRATCH_RECLAMATION: &str = "external scratch reclamation";

fn cancel_clean() -> Vec<Value> {
    vec![
        json!({"name":"exception","context":{"type":"cancel","severity":0}}),
        json!({"name":"clean"}),
    ]
}

fn tree_snapshot(path: &Path) -> Value {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Value::Null;
    };
    let mut node =
        json!({"mode": meta.mode(), "ino": meta.ino(), "uid": meta.uid(), "len": meta.len()});
    if meta.file_type().is_symlink() {
        node["link"] = json!(fs::read_link(path).unwrap());
    } else if meta.is_dir() {
        let mut children: Vec<PathBuf> = fs::read_dir(path)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        children.sort();
        node["children"] = children
            .iter()
            .map(|child| {
                json!([
                    child.file_name().unwrap().to_string_lossy(),
                    tree_snapshot(child)
                ])
            })
            .collect();
    } else if meta.len() <= 4096 {
        node["bytes"] = json!(fs::read(path).unwrap());
    }
    node
}

impl Fixture {
    /// InJob fixture whose cleanup helper runs the production exact-root
    /// observation and guarded reclamation over a private, test-owned root.
    fn no_grant_scratch() -> Self {
        let mut f = Fixture::new();
        f.podman(ContainerMode::InJob, true);
        let scratch = tempfile::tempdir().unwrap();
        fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
        f.use_scratch_root(scratch.path().to_owned());
        f.scratch = Some(scratch);
        f
    }

    fn use_scratch_root(&mut self, root: PathBuf) {
        self.request.scratch_root = root;
        self.intent.request_digest = digest(&serde_json::to_vec(&self.request).unwrap());
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        fs::write(
            self.root.path().join("action-fixture.json"),
            json!({ "source": source }).to_string(),
        )
        .unwrap();
    }

    fn leaf(&self) -> PathBuf {
        self.request.scratch_root.join("fABC")
    }

    fn make_leaf(&self) -> PathBuf {
        let leaf = self.leaf();
        fs::create_dir(&leaf).unwrap();
        fs::set_permissions(&leaf, fs::Permissions::from_mode(0o700)).unwrap();
        leaf
    }

    fn scratch_control(&self, control: &Value) {
        fs::write(
            self.intent.output_dir.join("scratch-control.json"),
            control.to_string(),
        )
        .unwrap();
    }

    fn cancel(&self) {
        let frames = self.call(
            json!({"op":"cancel","owner":self.intent.owner,"correlation":self.intent.correlation}),
        );
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    }

    fn debt_entries(&self) -> Vec<String> {
        let dir = self
            .options
            .journal_root
            .join("cleanup-debt")
            .join(&self.request.runtime.manifest_digest);
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn scratch_inputs(&self) -> Vec<Value> {
        fs::read_dir(&self.intent.output_dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("scratch-absence-") && name.ends_with(".input.json")
            })
            .map(|entry| serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap())
            .collect()
    }

    /// Submit a fresh owner of the same runtime through the same journal. The
    /// fixture native transport shares one jobspec file, so restore it after.
    fn submit_distinct_owner(&self) -> Value {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let mut intent = self.intent.clone();
        intent.correlation = format!(
            "{}-next{}",
            self.intent.correlation,
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        intent.output_dir = self.intent.output_dir.with_file_name(&intent.correlation);
        let spec = self.root.path().join("jobspec.json");
        let retained = fs::read(&spec).ok();
        let frames = self.call(json!({"op":"submit","request":self.request,"intent":intent}));
        if let Some(bytes) = retained {
            fs::write(&spec, bytes).unwrap();
        }
        let _ = fs::remove_dir_all(&intent.output_dir);
        frames.last().unwrap().clone()
    }

    fn assert_next_owner_held(&self) {
        let frame = self.submit_distinct_owner();
        assert_eq!(frame["kind"], "error", "{frame}");
        assert!(
            frame["message"]
                .as_str()
                .unwrap()
                .contains("unresolved runtime cleanup debt"),
            "{frame}"
        );
    }

    fn assert_next_owner_accepted(&self) {
        let frame = self.submit_distinct_owner();
        assert_eq!(frame["result"]["submission"]["kind"], "accepted", "{frame}");
    }

    /// Terminal committed without a grant: the receipt carries the exact helper
    /// proof, every debt is retired, and the next owner dispatches.
    fn assert_no_grant_terminal(&self, frames: &[Value], ending: &str, reclaimed: bool) {
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
            ending,
            "{frames:?}"
        );
        let state = self.state();
        assert!(state.allocation_grant.is_none());
        assert!(state.runtime_ownerships.is_empty() && state.container_launches.is_empty());
        let out = &self.intent.output_dir;
        let receipt: Value =
            serde_json::from_slice(&fs::read(out.join("receipt.json")).unwrap()).unwrap();
        let names: Vec<String> = receipt["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|reference| {
                Path::new(reference["path"].as_str().unwrap())
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let input = names
            .iter()
            .find(|name| name.starts_with("scratch-absence-") && name.ends_with(".input.json"))
            .unwrap_or_else(|| panic!("{names:?}"));
        let nonce = input
            .strip_prefix("scratch-absence-")
            .and_then(|rest| rest.strip_suffix(".input.json"))
            .unwrap();
        for suffix in ["input.json", "stdout", "stderr", "status.json"] {
            let name = format!("scratch-absence-{nonce}.{suffix}");
            assert!(names.contains(&name), "{name} not in {names:?}");
        }
        let input: Value = serde_json::from_slice(&fs::read(out.join(input)).unwrap()).unwrap();
        assert_eq!(input["host"], "dl380g10", "{input}");
        assert_eq!(input["reclamation"], SCRATCH_RECLAMATION, "{input}");
        let observed: Value = serde_json::from_slice(
            &fs::read(out.join(format!("scratch-absence-{nonce}.stdout"))).unwrap(),
        )
        .unwrap();
        assert_eq!(observed["observation"], json!({"kind":"absent","errno":2}));
        assert_eq!(observed["uid"], 1000);
        assert_eq!(observed["path"], json!(self.leaf()));
        let manifest = "scratch-reclamation.manifest.json".to_owned();
        let completion = "scratch-reclamation.complete.json".to_owned();
        if reclaimed {
            // The existing guarded flow inventoried, removed and re-proved absence.
            assert_eq!(observed["attribution"], SCRATCH_RECLAMATION, "{observed}");
            assert!(observed["generation_pre_removal"].is_object(), "{observed}");
            assert!(names.contains(&manifest) && names.contains(&completion));
            let retained: Value =
                serde_json::from_slice(&fs::read(out.join(&manifest)).unwrap()).unwrap();
            assert_eq!(retained["attribution"], SCRATCH_RECLAMATION);
            assert_eq!(retained["path"], json!(self.leaf()));
        } else {
            // Never present: observation only, nothing reclaimed.
            assert!(observed["attribution"].is_null() && observed["manifest"].is_null());
            assert!(!names.contains(&manifest) && !out.join(&manifest).exists());
        }
        assert!(fs::symlink_metadata(self.leaf()).is_err());
        assert!(self.debt_entries().is_empty(), "{:?}", self.debt_entries());
        self.assert_next_owner_accepted();
    }

    /// Held exactly as before: no receipt, no reclamation, debt retained and the
    /// next owner refused.
    fn assert_scratch_held(&self, frames: &[Value], scratch_debt: bool) {
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{frames:?}"
        );
        let out = &self.intent.output_dir;
        assert!(!out.join("receipt.json").exists());
        assert!(!out.join("scratch-reclamation.manifest.json").exists());
        assert!(!out.join("scratch-reclamation.complete.json").exists());
        let debts = self.debt_entries();
        assert!(!debts.is_empty());
        assert_eq!(
            debts.contains(&format!("{}.scratch", self.intent.correlation)),
            scratch_debt,
            "{debts:?}"
        );
        self.assert_next_owner_held();
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_cancel_while_pending_reaches_terminal_and_releases_dispatch() {
    let f = Fixture::no_grant_scratch();
    assert_eq!(f.submit()["kind"], "accepted");
    let cpus = f.write_fixture_allocation();
    let observed = f.observe_fixture_allocation(&cpus);
    assert_eq!(observed.last().unwrap()["kind"], "result", "{observed:?}");
    f.cancel();
    assert!(
        f.debt_entries()
            .contains(&format!("{}.unregistered", f.intent.correlation))
    );
    // After cancel intent no allocation can ever be observed or granted.
    let refused = f.observe_fixture_allocation(&cpus);
    assert_eq!(refused.last().unwrap()["kind"], "error", "{refused:?}");
    let frames = f.call(json!({"op":"validate_allocation","owner":f.intent.owner,
        "correlation":f.intent.correlation,"facts":observed.last().unwrap()["result"]["facts"]}));
    assert_eq!(frames.last().unwrap()["kind"], "error", "{frames:?}");
    assert!(f.state().allocation_grant.is_none() && f.state().cancel_requested);
    f.events(&cancel_clean());
    let frames = f.attach();
    f.assert_no_grant_terminal(&frames, "cancelled", false);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_owned_leaf_is_reclaimed_reproven_and_released() {
    for (contents, cancelled) in [("empty", false), ("nested", false), ("nested", true)] {
        let f = Fixture::no_grant_scratch();
        assert_eq!(f.submit()["kind"], "accepted");
        let leaf = f.make_leaf();
        if contents == "nested" {
            fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
            fs::create_dir_all(leaf.join("tl3-checks-fixture/head")).unwrap();
            fs::write(
                leaf.join("tl3-checks-fixture/head/subject.txt"),
                b"exported head",
            )
            .unwrap();
        }
        if cancelled {
            f.cancel();
        }
        f.events(&if cancelled {
            cancel_clean()
        } else {
            completed(256)
        });
        let frames = f.attach();
        f.assert_no_grant_terminal(
            &frames,
            if cancelled { "cancelled" } else { "infra-lost" },
            true,
        );
    }
}

/// Remove a negative case's injected leaf; only this fixture's own tree.
fn release_leaf(leaf: &Path) {
    match fs::symlink_metadata(leaf) {
        Ok(meta) if meta.is_dir() => {
            fs::set_permissions(leaf, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_dir_all(leaf).unwrap();
        }
        Ok(_) => fs::remove_file(leaf).unwrap(),
        Err(_) => {}
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_unsafe_leaf_holds_without_mutation() {
    // A symlinked approved root refuses at the O_NOFOLLOW parent walk.
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    let scratch = tempfile::tempdir().unwrap();
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let real = scratch.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = scratch.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    f.use_scratch_root(link);
    f.scratch = Some(scratch);
    assert_eq!(f.submit()["kind"], "accepted");
    let leaf = f.make_leaf();
    fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
    let before = tree_snapshot(&real);
    f.events(&completed(256));
    let frames = f.attach();
    f.assert_scratch_held(&frames, true);
    assert_eq!(tree_snapshot(&real), before);

    // Every other refusal is held before any manifest, so one fixture replays them.
    let f = Fixture::no_grant_scratch();
    assert_eq!(f.submit()["kind"], "accepted");
    f.events(&completed(256));
    let leaf = f.leaf();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("target"), b"outside bytes").unwrap();
    for case in [
        "symlink-leaf",
        "symlink-child",
        "group-writable",
        "regular-file",
        "over-entries",
        "over-bytes",
        "wrong-host",
    ] {
        match case {
            "symlink-leaf" => std::os::unix::fs::symlink(outside.path(), &leaf).unwrap(),
            "regular-file" => fs::write(&leaf, b"not a directory").unwrap(),
            _ => {
                f.make_leaf();
                fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
            }
        }
        match case {
            "symlink-child" => {
                std::os::unix::fs::symlink(outside.path().join("target"), leaf.join("link"))
                    .unwrap();
            }
            "group-writable" => {
                fs::set_permissions(&leaf, fs::Permissions::from_mode(0o770)).unwrap();
            }
            "over-entries" => {
                for index in 0..4097 {
                    fs::write(leaf.join(format!("entry-{index}")), b"").unwrap();
                }
            }
            "over-bytes" => {
                // Apparent size counts toward the 2 GiB bound; the file stays sparse.
                fs::File::create(leaf.join("sparse"))
                    .unwrap()
                    .set_len(2 * 1024 * 1024 * 1024 + 1)
                    .unwrap();
            }
            "wrong-host" => f.scratch_control(&json!({"nodename":"sparky"})),
            _ => {}
        }
        let before = (tree_snapshot(&leaf), tree_snapshot(outside.path()));
        let frames = f.attach();
        f.assert_scratch_held(&frames, true);
        assert_eq!(
            (tree_snapshot(&leaf), tree_snapshot(outside.path())),
            before,
            "{case}"
        );
        release_leaf(&leaf);
        let _ = fs::remove_file(f.intent.output_dir.join("scratch-control.json"));
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_off_placement_and_own_scope_never_reclaim() {
    // Any host other than the first-cut placement is refused before dispatch, so
    // a grantless scratch decision can never be reached for it.
    let mut f = Fixture::no_grant_scratch();
    let leaf = f.make_leaf();
    fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
    let before = tree_snapshot(&leaf);
    f.request.runtime.host_dependency.as_mut().unwrap().host = "sparky".into();
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    let refused = f.submit();
    assert_eq!(refused["kind"], "refused", "{refused}");
    assert_eq!(refused["terminal"]["ending"], "refused", "{refused}");
    assert!(!f.calls().contains("job submit "));
    assert!(f.scratch_inputs().is_empty());
    assert_eq!(tree_snapshot(&leaf), before);

    // OwnScope without a recorded execution is never all stopped, so it holds
    // before the scratch decision and its leaf is untouched.
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    let scratch = tempfile::tempdir().unwrap();
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    f.use_scratch_root(scratch.path().to_owned());
    f.scratch = Some(scratch);
    assert_eq!(f.submit()["kind"], "accepted");
    let leaf = f.make_leaf();
    fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
    let before = tree_snapshot(&leaf);
    f.events(&completed(256));
    let frames = f.attach();
    f.assert_scratch_held(&frames, false);
    assert!(f.state().allocation_grant.is_none());
    assert!(f.scratch_inputs().is_empty());
    assert_eq!(tree_snapshot(&leaf), before);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_helper_refusals_hold_debt_and_dispatch() {
    let f = Fixture::no_grant_scratch();
    assert_eq!(f.submit()["kind"], "accepted");
    f.events(&completed(256));
    for control in [
        json!({"replace":{"uid":0}}),
        json!({"replace":{"observation":{"kind":"absent","errno":13}}}),
        json!({"replace":{"observation":{"kind":"unknown","reason":"fixture refusal"}}}),
        json!({"replace":{"host":"sparky"}}),
        json!({"nodename":"sparky"}),
        json!({"replace":{"path":"/tmp/foreign"}}),
        json!({"exit":3}),
    ] {
        f.scratch_control(&control);
        let frames = f.attach();
        f.assert_scratch_held(&frames, true);
    }
    fs::remove_file(f.intent.output_dir.join("scratch-control.json")).unwrap();
    let frames = f.attach();
    f.assert_no_grant_terminal(&frames, "infra-lost", false);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_without_clean_or_with_executions_holds() {
    let f = Fixture::no_grant_scratch();
    assert_eq!(f.submit()["kind"], "accepted");
    let leaf = f.make_leaf();
    fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
    let before = tree_snapshot(&leaf);
    // Without native clean the watcher reports unknown; no scratch proof is attempted.
    f.events(&completed(256)[..2]);
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{frames:?}"
    );
    assert!(f.scratch_inputs().is_empty());
    assert!(!f.intent.output_dir.join("receipt.json").exists());
    // Any recorded execution, then also bound ownership, keeps grantless scratch held.
    let owned = f.container_ownership();
    f.events(&completed(256));
    let frames = f.attach();
    f.assert_scratch_held(&frames, false);
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,
        "correlation":f.intent.correlation,"ownership":owned}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let frames = f.attach();
    f.assert_scratch_held(&frames, false);
    assert!(f.state().allocation_grant.is_none());
    assert!(f.scratch_inputs().is_empty());
    assert_eq!(tree_snapshot(&leaf), before);

    // A retained grant keeps the grant path: an ordinary ending never reclaims.
    let f = Fixture::no_grant_scratch();
    assert_eq!(f.submit()["kind"], "accepted");
    f.retain_fixture_allocation();
    let leaf = f.make_leaf();
    fs::write(leaf.join("owned"), b"runner scratch bytes").unwrap();
    let before = tree_snapshot(&leaf);
    f.events(&completed(256));
    let frames = f.attach();
    f.assert_scratch_held(&frames, true);
    assert!(
        f.scratch_inputs()
            .iter()
            .all(|input| input["reclamation"].is_null())
    );
    assert_eq!(tree_snapshot(&leaf), before);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn no_grant_terminal_replay_is_refused() {
    let f = Fixture::no_grant_scratch();
    assert_eq!(f.submit()["kind"], "accepted");
    f.events(&completed(256));
    f.scratch_control(&json!({"exit":3}));
    let frames = f.attach();
    f.assert_scratch_held(&frames, true);
    let first = f.scratch_inputs();
    assert_eq!(first.len(), 1);
    // A replayed earlier helper result never satisfies a fresh nonce.
    f.scratch_control(&json!({"replace":{"nonce":first[0]["nonce"]}}));
    let frames = f.attach();
    f.assert_scratch_held(&frames, true);
    fs::remove_file(f.intent.output_dir.join("scratch-control.json")).unwrap();
    let frames = f.attach();
    f.assert_no_grant_terminal(&frames, "infra-lost", false);
    let receipt = fs::read(f.intent.output_dir.join("receipt.json")).unwrap();
    let attempts = f.scratch_inputs().len();
    let submits = f
        .calls()
        .lines()
        .filter(|line| line.starts_with("job submit"))
        .count();
    let again = f.attach();
    assert_eq!(
        again.last().unwrap()["result"]["observation"]["terminal"],
        frames.last().unwrap()["result"]["observation"]["terminal"]
    );
    assert_eq!(
        fs::read(f.intent.output_dir.join("receipt.json")).unwrap(),
        receipt
    );
    assert_eq!(f.scratch_inputs().len(), attempts);
    assert_eq!(f.submit()["kind"], "accepted");
    assert_eq!(
        f.calls()
            .lines()
            .filter(|line| line.starts_with("job submit"))
            .count(),
        submits
    );
    assert!(f.debt_entries().is_empty());
}

#[test]
#[ignore = "requires admitted PB actual-action fixture"]
fn no_grant_real_runner_paths_reach_terminal_and_release_dispatch() {
    // Sequential, so these actions never contend with each other for the fleet slot.
    // Killed after creating job scratch and before its workspace: an empty leaf.
    let f = generated_action_case("die-after-scratch-mkdir");
    assert!(fs::read_dir(f.leaf()).unwrap().next().is_none());
    f.events(&completed(256));
    let frames = f.attach();
    f.assert_no_grant_terminal(&frames, "infra-lost", true);
    drop(f);
    // Killed at its first allocation request, after exporting the checks workspace.
    let f = generated_action_case("die-before-allocation");
    assert!(fs::read_dir(f.leaf()).unwrap().flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with("tl3-checks-")
    }));
    f.events(&completed(256));
    let frames = f.attach();
    f.assert_no_grant_terminal(&frames, "infra-lost", true);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn job_id_spellings_share_one_cli_identity_and_refuse_other_ids() {
    let f = Fixture::new();
    assert_eq!(f.submit()["kind"], "accepted");
    // fABC is 9*58^2 + 10*58 + 11 = 30867 in Flux's Bitcoin base58 alphabet.
    for spelling in ["fABC", "ƒABC", "30867"] {
        let frames = f.call(json!({"op":"launch_context","owner":f.intent.owner,"correlation":f.intent.correlation,"job":spelling}));
        let last = frames.last().unwrap();
        assert_eq!(last["kind"], "result", "{spelling}: {frames:?}");
        assert_eq!(last["result"]["context"]["run"]["job"], "fABC");
        assert_eq!(
            last["result"]["context"]["scratch"]["leaf"],
            json!(f.request.scratch_root.join("fABC"))
        );
    }
    for spelling in [
        "fABD",
        "30868",
        "",
        "f0",
        "ƒ",
        "18446744073709551616",
        "fzzzzzzzzzzzzzzzz",
        "-1",
    ] {
        let frames = f.call(json!({"op":"launch_context","owner":f.intent.owner,"correlation":f.intent.correlation,"job":spelling}));
        assert_eq!(
            frames.last().unwrap()["kind"],
            "error",
            "{spelling}: {frames:?}"
        );
    }
    let state = f
        .options
        .journal_root
        .join(&f.intent.correlation)
        .join("state.json");
    let mut legacy: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    legacy["binding"]["id"]["job"] = json!("ƒABC");
    fs::write(&state, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let frames = f.call(json!({"op":"launch_context","owner":f.intent.owner,"correlation":f.intent.correlation,"job":"30867"}));
    assert_eq!(
        frames.last().unwrap()["result"]["context"]["run"]["job"],
        "fABC"
    );
}

#[test]
fn job_id_domain_is_checked_u64_and_canonical_ascii() {
    for (spelling, expected) in [
        ("0", "f1"),
        ("1", "f2"),
        ("ƒABC", "fABC"),
        ("30867", "fABC"),
        ("18446744073709551615", "fjpXCZedGfVQ"),
    ] {
        // Exercise the public wire boundary, including legacy records.
        let id: BackendId = serde_json::from_value(json!({"backend":"flux","instance":"fixture-generation","uri":"local:///fixture","job":spelling})).unwrap();
        assert_eq!(id.job, expected);
        assert_eq!(serde_json::to_value(&id).unwrap()["job"], expected);
    }
    for spelling in [
        "f0",
        "ƒ",
        "18446744073709551616",
        "fzzzzzzzzzzzzzzzz",
        "-1",
        "+1",
        " 1",
        "1\n",
    ] {
        assert!(serde_json::from_value::<BackendId>(json!({"backend":"flux","instance":"fixture-generation","uri":"local:///fixture","job":spelling})).is_err(), "{spelling}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn decimal_job_id_matches_retained_f58_at_real_cli() {
    let f = Fixture::new();
    assert_eq!(f.submit()["kind"], "accepted");
    let frames = f.call(json!({"op":"launch_context","owner":f.intent.owner,"correlation":f.intent.correlation,"job":"30867"}));
    assert_eq!(
        frames.last().unwrap()["result"]["context"]["run"]["job"],
        "fABC",
        "{frames:?}"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn native_submit_recovery_and_liveness_normalize_job_ids() {
    for spelling in ["fABC", "ƒABC", "30867"] {
        let f = Fixture::new();
        fs::write(f.root.path().join("submit-id"), format!("{spelling}\n")).unwrap();
        let submission = f.submit();
        assert_eq!(submission["kind"], "accepted", "{submission}");
        assert_eq!(submission["binding"]["id"]["job"], "fABC");
        let state = f
            .options
            .journal_root
            .join(&f.intent.correlation)
            .join("state.json");
        let mut retained: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
        assert_eq!(retained["binding"]["id"]["job"], "fABC");
        retained["binding"] = Value::Null;
        fs::write(&state, serde_json::to_vec(&retained).unwrap()).unwrap();
        fs::write(
            f.root.path().join("jobs.json"),
            json!({"jobs":[{"name":f.intent.correlation,"jobid":spelling}]}).to_string(),
        )
        .unwrap();
        // No event stream: recovery must bind the exact retained original jobspec.
        f.events(&[]);
        f.attach();
        let retained: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
        assert_eq!(retained["binding"]["id"]["job"], "fABC");
        let calls = fs::read_to_string(f.root.path().join("calls")).unwrap();
        assert!(
            calls.contains("job info --original fABC jobspec"),
            "{calls}"
        );
        let numeric = f.call(json!({"op":"launch_context","owner":f.intent.owner,"correlation":f.intent.correlation,"job":"30867"}));
        assert_eq!(numeric.last().unwrap()["kind"], "result");
        fs::write(
            f.root.path().join("jobs.json"),
            json!({"name":f.intent.correlation,"jobid":30867,"state":"RUN"}).to_string(),
        )
        .unwrap();
        let numeric = f.call(
            json!({"op":"liveness","owner":f.intent.owner,"correlation":f.intent.correlation}),
        );
        assert_eq!(
            numeric.last().unwrap()["result"]["liveness"]["authority"],
            "fresh_native",
            "{numeric:?}"
        );
        for job in [spelling, "fABD"] {
            fs::write(
                f.root.path().join("jobs.json"),
                json!({"name":f.intent.correlation,"jobid":job,"state":"RUN"}).to_string(),
            )
            .unwrap();
            let frames = f.call(
                json!({"op":"liveness","owner":f.intent.owner,"correlation":f.intent.correlation}),
            );
            assert_eq!(
                frames.last().unwrap()["result"]["liveness"]["authority"],
                if job == "fABD" {
                    "unknown"
                } else {
                    "fresh_native"
                },
                "{frames:?}"
            );
        }
    }
}

/// FX-GEN-PREP: real generator producer -> real host collector -> Rust collector,
/// terminal and scratch closure. Only host identity and native Flux are fixtures.
fn generator_prep_case(mode: &str) -> (Fixture, Value) {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    let scratch = tempfile::tempdir().unwrap();
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    f.request.scratch_root = scratch.path().to_owned();
    f.request.reservation.cpus = 1.try_into().unwrap();
    f.request.reservation.parallelism = 1.try_into().unwrap();
    f.request.suite = Suite::Generator {
        input_digest: "a".repeat(64),
    };
    f.request.no_progress_seconds = Some(10.try_into().unwrap());
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    f.submit();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let mut environment = f.request.environment.clone();
    for (name, value) in [
        ("TL3_EXECUTION_BACKEND", "flux".to_owned()),
        (
            "TL3_RUN_BACKEND_CLI",
            env!("CARGO_BIN_EXE_tl3-run-backend").to_owned(),
        ),
        (
            "TL3_RUN_BACKEND_CONFIG",
            f.config.to_string_lossy().into_owned(),
        ),
        ("TL3_RUN_OWNER", f.intent.owner.clone()),
        ("TL3_RUN_CORRELATION", f.intent.correlation.clone()),
        (
            "TL3_RUN_DIAGNOSTIC_DIR",
            f.intent.output_dir.to_string_lossy().into_owned(),
        ),
        ("FLUX_JOB_ID", "fABC".to_owned()),
        ("TL3_RUN_BACKEND_SESSION", "1".to_owned()),
    ] {
        environment.insert(name.into(), value);
    }
    fs::write(
        f.root.path().join("action-fixture.json"),
        serde_json::to_vec(&json!({
            "environment": environment, "output_dir": f.intent.output_dir,
            "scratch": scratch.path().join("fABC"), "mode": mode, "source": source,
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new("/home/rob/venvs/pb-cpu/bin/python")
        .arg(source.join("harness/tests/fx_generator_prep_e2e.py"))
        .arg(f.root.path())
        .arg(&source)
        .arg(mode)
        .output()
        .unwrap();
    eprintln!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    f.scratch = Some(scratch);
    assert!(output.status.success(), "generator driver crashed");
    let driver: Value = serde_json::from_slice(
        &fs::read(f.intent.output_dir.join("generator-driver.json")).unwrap(),
    )
    .unwrap();
    (f, driver)
}

fn retain_generator_evidence(f: &Fixture, name: &str, frames: &[Value]) {
    if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
        let destination = Path::new(&destination).join(name);
        fs::create_dir_all(&destination).unwrap();
        for entry in fs::read_dir(&f.intent.output_dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        let state = f
            .options
            .journal_root
            .join(&f.intent.correlation)
            .join("state.json");
        if state.is_file() {
            fs::copy(state, destination.join("state.json")).unwrap();
        }
        fs::write(
            destination.join("attach-frames.json"),
            serde_json::to_vec(frames).unwrap(),
        )
        .unwrap();
    }
}

fn generator_frame(f: &Fixture) -> Value {
    let stdout = fs::read_to_string(f.intent.output_dir.join("host.stdout")).unwrap();
    let lines: Vec<_> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("TL3_GENERATOR_ARTIFACT "))
        .collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    serde_json::from_str(lines[0]).unwrap()
}

fn cleanup_debts(f: &Fixture) -> Vec<String> {
    let dir = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(&f.intent.correlation))
                .collect()
        })
        .unwrap_or_default()
}

/// A distinct owner dispatching on the same runtime manifest through the real
/// unresolved-debt gate. Returns the submit frames; its output is removed.
fn distinct_owner_submit(f: &Fixture, tag: &str) -> Vec<Value> {
    let binding = f.state().binding.unwrap();
    fs::copy(
        f.intent.output_dir.join("original-jobspec.json"),
        f.root
            .path()
            .join(format!("jobspec-{}.json", binding.id.job)),
    )
    .unwrap();
    let submit_id = f.root.path().join("submit-id");
    let previous_id = fs::read(&submit_id).unwrap();
    fs::write(&submit_id, "fDEF\n").unwrap();
    let mut intent = f.intent.clone();
    intent.owner = format!("fixture-owner-{tag}");
    intent.correlation = format!("{}-{tag}", f.intent.correlation);
    intent.output_dir = Path::new("/mnt/shared/flux-out/tl3").join(&intent.correlation);
    let frames = f.call(json!({"op":"submit","request":f.request,"intent":intent}));
    fs::write(submit_id, previous_id).unwrap();
    let _ = fs::remove_dir_all(&intent.output_dir);
    frames
}

fn unrelated_debt(f: &Fixture) -> PathBuf {
    let dir = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest);
    fs::create_dir_all(&dir).unwrap();
    let mut intent = f.intent.clone();
    intent.owner = "unrelated-owner".into();
    intent.correlation = "unrelated-correlation-out".into();
    let path = dir.join("unrelated-correlation-out.unregistered");
    fs::write(&path, serde_json::to_vec(&intent).unwrap()).unwrap();
    path
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generator_preparation_failure_closes_typed_failed_through_real_collector() {
    let (mode, operation) = ("prep-failure", "input_manifest");
    {
        let (f, driver) = generator_prep_case(mode);
        assert!(driver["error_type"].is_null(), "{mode}: {driver}");
        assert_eq!(driver["code"], 0, "{mode}: {driver}");
        assert_eq!(driver["scratch_absent"], true, "{mode}: {driver}");
        let state = f.state();
        assert!(state.allocation_grant.is_some(), "{mode}");
        assert!(state.container_launches.is_empty() && state.runtime_ownerships.is_empty());
        let produced = generator_frame(&f);
        assert_eq!(produced["status"], "failed", "{mode}: {produced}");
        assert_eq!(produced["phase"], "preparation", "{mode}: {produced}");
        assert_eq!(produced["timings"]["container"]["outcome"], "skipped");
        assert_eq!(
            produced["cause"]["operation"], operation,
            "{mode}: {produced}"
        );
        // The unrelated owner's debt must survive this owner's closure.
        let unrelated = (mode == "readonly-checkout").then(|| unrelated_debt(&f));
        f.events(&completed(0));
        let frames = f.attach();
        retain_generator_evidence(&f, &format!("generator-{mode}"), &frames);
        let terminal = frames.last().unwrap()["result"]["observation"]["terminal"].clone();
        // A typed generator failure is never a pass, even when native exits zero.
        assert_eq!(terminal["ending"], "failed", "{mode}: {frames:?}");
        let receipt: Value =
            serde_json::from_slice(&fs::read(f.intent.output_dir.join("receipt.json")).unwrap())
                .unwrap();
        assert!(
            receipt["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["path"].as_str().unwrap().contains("/scratch-absence-")),
            "{receipt}"
        );
        assert!(cleanup_debts(&f).is_empty(), "{:?}", cleanup_debts(&f));
        let next = distinct_owner_submit(&f, "next");
        if let Some(unrelated) = &unrelated {
            assert!(unrelated.exists(), "closure cleared another owner's debt");
            assert_eq!(next.last().unwrap()["kind"], "error", "{next:?}");
            assert!(
                json!(next)
                    .to_string()
                    .contains("unresolved runtime cleanup debt"),
                "{next:?}"
            );
        } else {
            // A distinct owner on the same runtime passes the real dispatch gate.
            assert_eq!(
                next.last().unwrap()["result"]["submission"]["kind"],
                "accepted",
                "{next:?}"
            );
        }
        let repeated = f.attach();
        assert_eq!(
            repeated.last().unwrap()["result"]["observation"]["terminal"],
            terminal,
            "{repeated:?}"
        );
        let result = f.call(json!({"op":"worker_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
        let reference: BlobRef =
            serde_json::from_value(result.last().unwrap()["result"]["payload"]["artifact"].clone())
                .unwrap_or_else(|_| panic!("{mode}: {result:?}"));
        // Producer bytes on host stdout are exactly the bytes the worker receives.
        let stdout = fs::read_to_string(f.intent.output_dir.join("host.stdout")).unwrap();
        let line = stdout
            .lines()
            .find_map(|line| line.strip_prefix("TL3_GENERATOR_ARTIFACT "))
            .unwrap();
        let bytes = fs::read(&reference.path).unwrap();
        assert_eq!(bytes, line.as_bytes());
        assert!(!String::from_utf8_lossy(&bytes).contains("ghp_"));
        // A replayed frame after terminal is refused.
        let replay = collect_worker(&f, stdout.as_bytes());
        assert_eq!(replay.last().unwrap()["kind"], "error", "{replay:?}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generator_no_container_failure_refuses_forged_and_unproven_closure() {
    let (f, driver) = generator_prep_case("prep-failure");
    assert!(driver["error_type"].is_null(), "{driver}");
    let produced = generator_frame(&f);
    let frame = |value: &Value| format!("TL3_GENERATOR_ARTIFACT {value}\n");
    let mut variants = vec![("malformed", "TL3_GENERATOR_ARTIFACT {\n".to_owned())];
    let mut forged = produced.clone();
    forged["status"] = json!("completed");
    forged["exit_code"] = json!(0);
    forged["phase"] = json!("completed");
    forged["output_manifest"] = json!("c".repeat(64));
    forged["outputs"] = json!([]);
    variants.push(("forged-success", frame(&forged)));
    let mut begun = produced.clone();
    begun["timings"]["container"]["outcome"] = json!("failed");
    variants.push(("container-begun", frame(&begun)));
    let mut phase = produced.clone();
    phase["phase"] = json!("container");
    variants.push(("container-phase", frame(&phase)));
    let mut exited = produced.clone();
    exited["exit_code"] = json!(1);
    variants.push(("exit-code", frame(&exited)));
    let mut secret = produced.clone();
    secret["cause"]["detail"] = json!(format!("token ghp_{}", "Q".repeat(36)));
    variants.push(("secret-cause", frame(&secret)));
    let mut multiline = produced.clone();
    multiline["message"] = json!("x\n".repeat(20_000));
    variants.push(("unbounded-message", frame(&multiline)));
    // Single-field mutations of the genuine frame: each pins exactly one clause of
    // generator_failed_before_container; without that clause the frame would collect.
    let mut log = produced.clone();
    log["log"] = json!("x");
    variants.push(("log-present", frame(&log)));
    let mut outputs = produced.clone();
    outputs["outputs"] = json!([]);
    variants.push(("outputs-present", frame(&outputs)));
    let mut manifest = produced.clone();
    manifest["output_manifest"] = json!("c".repeat(64));
    variants.push(("output-manifest-present", frame(&manifest)));
    let mut validated = produced.clone();
    validated["timings"]["validation"]["outcome"] = json!("completed");
    variants.push(("validation-completed", frame(&validated)));
    let mut prepared = produced.clone();
    prepared["timings"]["preparation"]["outcome"] = json!("completed");
    variants.push(("preparation-completed", frame(&prepared)));
    for (name, raw) in &variants {
        let result = collect_worker(&f, raw.as_bytes());
        assert_eq!(
            result.last().unwrap()["kind"],
            "error",
            "{name}: {result:?}"
        );
    }
    // A foreign run is refused by collect_worker's durable-binding check.
    let stdout = api::durable::blob(
        &f.intent.output_dir.join("raw-worker-stdout"),
        frame(&produced).as_bytes(),
    )
    .unwrap();
    let mut run = f.ownership().run;
    run.job = "fABD".into();
    let foreign = f.call(json!({"op":"collect_worker","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{
        "run":run,"stdout":stdout,"runtime_binding":null,"cleanup":null}}));
    assert_eq!(foreign.last().unwrap()["kind"], "error", "{foreign:?}");
    // A claimed runtime binding observation contradicts a no-container failure. The
    // run is exact, so refusal comes from the predicate/identity, not the binding check.
    let claimed = RuntimeBindingInput {
        execution: "fixture-execution".into(),
        container_id: None,
        run: f.ownership().run,
        request_digest: f.intent.request_digest.clone(),
        allocation: stdout.clone(),
        topology: stdout.clone(),
        host: ProcessPlacement {
            rank: 0,
            host: "dl380g10".into(),
            allowed_cpus: "0".into(),
            cgroup: "0::/user.slice/native-broker".into(),
        },
        container: None,
        container_scope: None,
    };
    let bound = f.call(json!({"op":"collect_worker","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{
        "run":f.ownership().run,"stdout":stdout,"runtime_binding":claimed,"cleanup":null}}));
    assert_eq!(bound.last().unwrap()["kind"], "error", "{bound:?}");
    assert_eq!(
        bound.last().unwrap()["message"],
        "worker frame structure, linkage or artifact bytes invalid",
        "{bound:?}"
    );
    // The genuine producer frame still collects while no execution exists ...
    assert_eq!(
        collect_worker(&f, frame(&produced).as_bytes())
            .last()
            .unwrap()["kind"],
        "result"
    );
    // ... but never once a container launch began. Register the launch through the
    // real durable prelaunch intent; the backend refuses ownership without one.
    let launcher = f.ownership();
    let begun = f.call(json!({"op":"begin_container_launch","owner":f.intent.owner,"correlation":f.intent.correlation,"launcher":launcher}));
    assert_eq!(begun.last().unwrap()["kind"], "result", "{begun:?}");
    assert_eq!(f.state().container_launches.len(), 1);
    let result = collect_worker(&f, frame(&produced).as_bytes());
    assert_eq!(result.last().unwrap()["kind"], "error", "{result:?}");

    // Closure negatives each start from a fresh real producer/collector run.
    for case in [
        "conflicting-native-exit",
        "missing-clean",
        "scratch-retained",
    ] {
        let (f, driver) = generator_prep_case("prep-failure");
        assert!(driver["error_type"].is_null(), "{case}: {driver}");
        match case {
            "conflicting-native-exit" => f.events(&completed(1)),
            "missing-clean" => f.events(&completed(0)[..2]),
            _ => {
                fs::create_dir(f.request.scratch_root.join("fABC")).unwrap();
                f.events(&completed(0));
            }
        }
        let frames = f.attach();
        retain_generator_evidence(&f, &format!("generator-negative-{case}"), &frames);
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{case}: {frames:?}"
        );
        assert!(!f.intent.output_dir.join("receipt.json").exists(), "{case}");
        if case == "scratch-retained" {
            assert!(f.request.scratch_root.join("fABC").exists());
            assert!(
                !cleanup_debts(&f).is_empty(),
                "{case}: scratch debt not retained"
            );
        }
        let next = distinct_owner_submit(&f, "next");
        if case == "conflicting-native-exit" {
            // Classification stays fail-closed (Unknown, no receipt), but cleanup is
            // proven: no executions, native clean, own scratch absent. No debt is
            // held for this owner and the runtime gate admits the next owner.
            assert!(
                cleanup_debts(&f).is_empty(),
                "{case}: {:?}",
                cleanup_debts(&f)
            );
            assert_eq!(
                next.last().unwrap()["result"]["submission"]["kind"],
                "accepted",
                "{case}: {next:?}"
            );
        } else {
            // Unproven cleanup keeps the runtime gate closed for every other owner.
            assert!(!cleanup_debts(&f).is_empty(), "{case}: debt not retained");
            assert_eq!(next.last().unwrap()["kind"], "error", "{case}: {next:?}");
            assert!(
                json!(next)
                    .to_string()
                    .contains("unresolved runtime cleanup debt"),
                "{case}: {next:?}"
            );
        }
    }
}

/// FLUX-GENERATOR-EXEC: a real generator on the owned test-job launch path -> real
/// host collector -> Rust collector, terminal and scratch closure. Only host
/// identity, the Podman executable and native Flux are fixtures.
fn generator_exec_case(mode: &str) -> (Fixture, Value) {
    let mut f = Fixture::new();
    f.podman(ContainerMode::InJob, true);
    let scratch = tempfile::tempdir().unwrap();
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    f.request.scratch_root = scratch.path().to_owned();
    f.request.reservation.cpus = 1.try_into().unwrap();
    f.request.reservation.parallelism = 1.try_into().unwrap();
    f.request.suite = Suite::Generator {
        input_digest: "a".repeat(64),
    };
    f.request.no_progress_seconds = Some(10.try_into().unwrap());
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    f.submit();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let mut environment = f.request.environment.clone();
    for (name, value) in [
        ("TL3_EXECUTION_BACKEND", "flux".to_owned()),
        (
            "TL3_RUN_BACKEND_CLI",
            env!("CARGO_BIN_EXE_tl3-run-backend").to_owned(),
        ),
        (
            "TL3_RUN_BACKEND_CONFIG",
            f.config.to_string_lossy().into_owned(),
        ),
        ("TL3_RUN_OWNER", f.intent.owner.clone()),
        ("TL3_RUN_CORRELATION", f.intent.correlation.clone()),
        (
            "TL3_RUN_DIAGNOSTIC_DIR",
            f.intent.output_dir.to_string_lossy().into_owned(),
        ),
        ("FLUX_JOB_ID", "fABC".to_owned()),
        ("TL3_RUN_BACKEND_SESSION", "1".to_owned()),
    ] {
        environment.insert(name.into(), value);
    }
    fs::write(
        f.root.path().join("action-fixture.json"),
        serde_json::to_vec(&json!({
            "environment": environment, "output_dir": f.intent.output_dir,
            "scratch": scratch.path().join("fABC"), "mode": mode, "source": source,
            "image": f.request.runtime.expected_image,
            "cleanup_budget_seconds": CLEANUP_HELPER_BUDGET_SECONDS,
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new("/home/rob/venvs/pb-cpu/bin/python")
        .arg(source.join("harness/tests/test_fx_generator_exec.py"))
        .arg(f.root.path())
        .arg(&source)
        .arg(mode)
        .output()
        .unwrap();
    eprintln!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    f.scratch = Some(scratch);
    assert!(output.status.success(), "generator exec driver crashed");
    let driver: Value = serde_json::from_slice(
        &fs::read(f.intent.output_dir.join("generator-exec-driver.json")).unwrap(),
    )
    .unwrap();
    // TERM may surface as a collector error; cancel asserts its own closure facts.
    assert!(
        mode == "cancel" || driver["error_type"].is_null(),
        "{mode}: {driver}"
    );
    assert_eq!(driver["payload_started"], true, "{mode}: {driver}");
    assert_eq!(driver["scratch_absent"], true, "{mode}: {driver}");
    let state = f.state();
    assert_eq!(state.container_launches.len(), 1, "{mode}");
    assert_eq!(state.runtime_ownerships.len(), 1, "{mode}");
    (f, driver)
}

fn generator_terminal(f: &Fixture, name: &str) -> Value {
    f.events(&completed(0));
    let frames = f.attach();
    retain_generator_evidence(f, name, &frames);
    let terminal = frames.last().unwrap()["result"]["observation"]["terminal"].clone();
    let repeated = f.attach();
    assert_eq!(
        repeated.last().unwrap()["result"]["observation"]["terminal"],
        terminal
    );
    terminal
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generator_exec_success_passes_through_real_collector_with_proven_cleanup() {
    let (f, driver) = generator_exec_case("success");
    assert_eq!(driver["code"], 0, "{driver}");
    let produced = generator_frame(&f);
    assert_eq!(produced["status"], "completed", "{produced}");
    assert_eq!(produced["exit_code"], 0, "{produced}");
    assert_eq!(
        produced["outputs"][0]["path"], "generated/out.txt",
        "{produced}"
    );
    let terminal = generator_terminal(&f, "generator-exec-success");
    assert_eq!(terminal["ending"], "passed", "{terminal}");
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.intent.output_dir.join("receipt.json")).unwrap())
            .unwrap();
    assert!(
        receipt["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"].as_str().unwrap().contains("/scratch-absence-")),
        "{receipt}"
    );
    assert!(cleanup_debts(&f).is_empty(), "{:?}", cleanup_debts(&f));
    let result = f.call(json!({"op":"worker_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    let reference: BlobRef =
        serde_json::from_value(result.last().unwrap()["result"]["payload"]["artifact"].clone())
            .unwrap_or_else(|_| panic!("{result:?}"));
    // Producer bytes on host stdout are exactly the bytes the worker receives.
    let stdout = fs::read_to_string(f.intent.output_dir.join("host.stdout")).unwrap();
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("TL3_GENERATOR_ARTIFACT "))
        .unwrap();
    assert_eq!(fs::read(&reference.path).unwrap(), line.as_bytes());
    let next = distinct_owner_submit(&f, "next");
    assert_eq!(
        next.last().unwrap()["result"]["submission"]["kind"],
        "accepted",
        "{next:?}"
    );
    // A replayed frame after terminal is refused.
    let replay = collect_worker(&f, stdout.as_bytes());
    assert_eq!(replay.last().unwrap()["kind"], "error", "{replay:?}");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generator_exec_failure_closes_typed_failed_and_keeps_unrelated_debt() {
    let (f, driver) = generator_exec_case("failure");
    assert_eq!(driver["code"], 0, "{driver}");
    let produced = generator_frame(&f);
    assert_eq!(produced["status"], "failed", "{produced}");
    assert_eq!(produced["exit_code"], 3, "{produced}");
    assert_eq!(produced["timings"]["container"]["outcome"], "failed");
    assert!(produced.get("outputs").is_none() && produced.get("output_manifest").is_none());
    // Moved from the prep readonly-checkout row: another owner's debt survives this
    // owner's closure and keeps the dispatch gate closed.
    let unrelated = unrelated_debt(&f);
    let terminal = generator_terminal(&f, "generator-exec-failure");
    // A typed generator failure is never a pass, even when native exits zero.
    assert_eq!(terminal["ending"], "failed", "{terminal}");
    assert!(cleanup_debts(&f).is_empty(), "{:?}", cleanup_debts(&f));
    assert!(unrelated.exists(), "closure cleared another owner's debt");
    let next = distinct_owner_submit(&f, "next");
    assert_eq!(next.last().unwrap()["kind"], "error", "{next:?}");
    assert!(
        json!(next)
            .to_string()
            .contains("unresolved runtime cleanup debt"),
        "{next:?}"
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generator_exec_cancel_unwinds_cleanup_without_artifact() {
    let (f, driver) = generator_exec_case("cancel");
    assert_eq!(driver["signals"].as_array().unwrap().len(), 1, "{driver}");
    let stdout = fs::read_to_string(f.intent.output_dir.join("host.stdout")).unwrap_or_default();
    assert!(!stdout.contains("TL3_GENERATOR_ARTIFACT "), "{stdout}");
    assert!(f.intent.output_dir.join("action-sigterm.json").is_file());
    finish_cancel_fixture(&f, false);
    let terminal = f.attach().last().unwrap()["result"]["observation"]["terminal"].clone();
    let result = f.call(json!({"op":"worker_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(result.last().unwrap()["kind"], "error", "{result:?}");
}

// FX-ATTACH-RACE: the job's runner observes its placement while the submit's acceptance
// is still unknown, before any controller attach has recovered the binding.
/// Submit whose reply is lost after dispatch: the intent is durable, the binding is not.
fn acceptance_unknown(f: &Fixture) {
    fs::write(f.root.path().join("mode"), "unknown").unwrap();
    assert_eq!(f.submit()["kind"], "unknown");
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    assert!(f.state().dispatched);
    assert!(f.state().binding.is_none());
}

/// The live native R graph for the single-host first-cut placement.
fn early_allocation(f: &Fixture) -> String {
    let count = f.request.reservation.cpus.get();
    let cpus = (0..count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let nodes = std::iter::once(json!({"metadata":{"type":"node","rank":0,"id":0,"name":"dl380g10"}}))
        .chain((0..count).map(|n| json!({"metadata":{"type":"core","rank":0,"id":n,"properties":{"os-pus":n.to_string()}}})))
        .collect::<Vec<_>>();
    fs::write(
        f.root.path().join("all.json"),
        json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":cpus}}],"nodelist":["dl380g10"]},"scheduling":{"graph":{"nodes":nodes}}}).to_string(),
    )
    .unwrap();
    cpus
}

/// The runner's own observe_allocation for `job` (its FLUX_JOB_ID), asserting nothing.
fn early_observe(f: &Fixture, job: &str) -> Vec<Value> {
    let cpus = early_allocation(f);
    let mut run = f.ownership().run;
    run.job = job.into();
    f.call(json!({"op":"observe_allocation","owner":f.intent.owner,"correlation":f.intent.correlation,
        "observed":{"run":run,"request_digest":f.intent.request_digest,"hostname":"dl380g10","allowed_cpus":cpus,
        "cgroup":format!("0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-{job}:sys.service")}}))
}

fn early_validate(f: &Fixture, facts: &Value) -> Vec<Value> {
    f.call(json!({"op":"validate_allocation","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":facts}))
}

fn native_calls(calls: &str, prefix: &str) -> usize {
    calls
        .lines()
        .filter(|line| line.starts_with(prefix))
        .count()
}

fn observation_blobs(f: &Fixture) -> usize {
    fs::read_dir(&f.intent.output_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("allocation-observation-")
                })
                .count()
        })
        .unwrap_or(0)
}

/// A same-name native job whose immutable original jobspec is not the retained one.
fn foreign_original(f: &Fixture, job: &str) {
    let mut spec: Value =
        serde_json::from_slice(&fs::read(f.root.path().join("jobspec.json")).unwrap()).unwrap();
    spec["attributes"]["system"]["duration"] = json!(1.0);
    fs::write(
        f.root.path().join(format!("jobspec-{job}.json")),
        spec.to_string(),
    )
    .unwrap();
}

fn listed_jobs(f: &Fixture, jobs: &[&str]) {
    let jobs = jobs
        .iter()
        .enumerate()
        .map(|(id, job)| json!({"name":f.intent.correlation,"jobid":job,"id":id + 42}))
        .collect::<Vec<_>>();
    fs::write(
        f.root.path().join("jobs.json"),
        json!({ "jobs": jobs }).to_string(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn acceptance_unknown_early_observe_recovers_exact_job_and_admits() {
    let f = Fixture::new();
    acceptance_unknown(&f);
    let observed = early_observe(&f, "fABC");
    assert_eq!(observed.last().unwrap()["kind"], "result", "{observed:?}");
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
    let calls = f.calls();
    assert_eq!(native_calls(&calls, "jobs -a "), 1, "{calls}");
    assert!(
        calls.contains("job info --original fABC jobspec"),
        "{calls}"
    );
    assert_eq!(native_calls(&calls, "job submit "), 1, "{calls}");
    let granted = early_validate(&f, &observed.last().unwrap()["result"]["facts"]);
    assert_eq!(granted.last().unwrap()["kind"], "result", "{granted:?}");
    assert_eq!(f.state().allocation_grant.unwrap().run.job, "fABC");
    f.report(ReportOutcome::Passed, true);
    f.events(&completed(0));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["terminal"]["ending"],
        "passed",
        "{frames:?}"
    );
    let calls = f.calls();
    assert_eq!(native_calls(&calls, "jobs -a "), 1, "{calls}");
    assert_eq!(native_calls(&calls, "job submit "), 1, "{calls}");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn early_observe_for_cancelled_terminal_or_undispatched_job_is_refused_without_recovery() {
    for case in ["cancelled", "undispatched", "terminal"] {
        let f = Fixture::new();
        if case == "cancelled" {
            acceptance_unknown(&f);
            // The cancel's own reconciliation cannot prove the job, so no binding exists.
            fs::write(f.root.path().join("mode"), "query-error").unwrap();
            f.call(
                json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}),
            );
            fs::write(f.root.path().join("mode"), "normal").unwrap();
            assert!(f.state().cancel_requested, "{case}");
            assert!(f.state().binding.is_none(), "{case}");
        } else {
            // The output directory already exists, so nothing is dispatched.
            fs::create_dir_all(&f.intent.output_dir).unwrap();
            assert_eq!(f.submit()["kind"], "unknown", "{case}");
            assert!(!f.state().dispatched, "{case}");
            if case == "terminal" {
                f.call(
                    json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}),
                );
                assert!(f.state().terminal.is_some(), "{case}");
            }
        }
        let before = f.calls().lines().count();
        let observed = early_observe(&f, "fABC");
        assert_eq!(
            observed.last().unwrap()["kind"],
            "error",
            "{case}: {observed:?}"
        );
        let after = f
            .calls()
            .lines()
            .skip(before)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(native_calls(&after, "jobs "), 0, "{case}: {after}");
        assert_eq!(native_calls(&after, "job info "), 0, "{case}: {after}");
        assert!(f.state().binding.is_none(), "{case}");
        assert!(f.state().allocation_grant.is_none(), "{case}");
        assert_eq!(observation_blobs(&f), 0, "{case}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn early_observe_without_matching_job_is_refused() {
    for case in ["no-candidate", "query-error", "foreign-only"] {
        let f = Fixture::new();
        acceptance_unknown(&f);
        match case {
            "no-candidate" => listed_jobs(&f, &[]),
            "query-error" => fs::write(f.root.path().join("mode"), "query-error").unwrap(),
            _ => {
                foreign_original(&f, "fZZZ");
                listed_jobs(&f, &["fZZZ"]);
            }
        }
        let observed = early_observe(&f, "fABC");
        assert_eq!(
            observed.last().unwrap()["kind"],
            "error",
            "{case}: {observed:?}"
        );
        assert!(f.state().binding.is_none(), "{case}");
        assert!(f.state().allocation_grant.is_none(), "{case}");
        assert_eq!(observation_blobs(&f), 0, "{case}");
        assert!(
            !f.calls().contains("job info fABC R"),
            "{case}: {}",
            f.calls()
        );
        let frames = f.attach();
        assert_eq!(
            frames.last().unwrap()["result"]["observation"]["kind"],
            "unknown",
            "{case}: {frames:?}"
        );
        assert!(f.state().binding.is_none(), "{case}");
        assert_eq!(native_calls(&f.calls(), "job submit "), 1, "{case}");
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn early_observe_with_conflicting_same_name_job_is_refused() {
    let f = Fixture::new();
    acceptance_unknown(&f);
    foreign_original(&f, "fDEF");
    listed_jobs(&f, &["fABC", "fDEF"]);
    let observed = early_observe(&f, "fABC");
    assert_eq!(observed.last().unwrap()["kind"], "error", "{observed:?}");
    let calls = f.calls();
    assert!(
        calls.contains("job info --original fABC jobspec"),
        "{calls}"
    );
    assert!(
        calls.contains("job info --original fDEF jobspec"),
        "{calls}"
    );
    assert!(f.state().binding.is_none());
    assert!(f.state().allocation_grant.is_none());
    assert_eq!(observation_blobs(&f), 0);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn early_observe_for_wrong_run_is_refused_after_exact_recovery() {
    let f = Fixture::new();
    acceptance_unknown(&f);
    let observed = early_observe(&f, "fDEF");
    assert_eq!(observed.last().unwrap()["kind"], "error", "{observed:?}");
    // Recovery bound only the exact original job, never the runner's claimed one.
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
    assert!(f.state().allocation_grant.is_none());
    assert!(!f.calls().contains("job info fDEF R"), "{}", f.calls());
    let exact = early_observe(&f, "fABC");
    assert_eq!(exact.last().unwrap()["kind"], "result", "{exact:?}");
    let mut forged = exact.last().unwrap()["result"]["facts"].clone();
    forged["run"]["job"] = json!("fDEF");
    let refused = early_validate(&f, &forged);
    assert_eq!(refused.last().unwrap()["kind"], "error", "{refused:?}");
    assert!(f.state().allocation_grant.is_none());
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn early_observe_replay_is_idempotent() {
    // Repeated observation reuses the durable binding and retains identical facts.
    let f = Fixture::new();
    acceptance_unknown(&f);
    let first = early_observe(&f, "fABC");
    assert_eq!(first.last().unwrap()["kind"], "result", "{first:?}");
    let second = early_observe(&f, "fABC");
    assert_eq!(second.last().unwrap()["kind"], "result", "{second:?}");
    assert_eq!(
        first.last().unwrap()["result"]["facts"],
        second.last().unwrap()["result"]["facts"]
    );
    assert_eq!(native_calls(&f.calls(), "jobs -a "), 1, "{}", f.calls());
    // A replayed submit returns the recovered binding and never resubmits.
    let replay = f.submit();
    assert_eq!(replay["kind"], "accepted", "{replay:?}");
    assert_eq!(replay["binding"]["id"]["job"], "fABC", "{replay:?}");
    assert_eq!(native_calls(&f.calls(), "job submit "), 1, "{}", f.calls());

    // Interruption after the binding commit: the native R read fails, then a retry
    // proceeds from the committed binding without recovering again.
    let f = Fixture::new();
    acceptance_unknown(&f);
    let cpus = early_allocation(&f);
    let graph = fs::read(f.root.path().join("all.json")).unwrap();
    fs::remove_file(f.root.path().join("all.json")).unwrap();
    let mut run = f.ownership().run;
    run.job = "fABC".into();
    let interrupted = f.call(json!({"op":"observe_allocation","owner":f.intent.owner,"correlation":f.intent.correlation,
        "observed":{"run":run,"request_digest":f.intent.request_digest,"hostname":"dl380g10","allowed_cpus":cpus,
        "cgroup":"0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service"}}));
    assert_eq!(
        interrupted.last().unwrap()["kind"],
        "error",
        "{interrupted:?}"
    );
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
    fs::write(f.root.path().join("all.json"), graph).unwrap();
    let resumed = early_observe(&f, "fABC");
    assert_eq!(resumed.last().unwrap()["kind"], "result", "{resumed:?}");
    assert_eq!(native_calls(&f.calls(), "jobs -a "), 1, "{}", f.calls());
    let granted = early_validate(&f, &resumed.last().unwrap()["result"]["facts"]);
    assert_eq!(granted.last().unwrap()["kind"], "result", "{granted:?}");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn concurrent_attach_and_early_observe_commit_one_binding() {
    let f = Fixture::new();
    acceptance_unknown(&f);
    fs::write(f.root.path().join("mode"), "held-query").unwrap();
    // Release the held query on every exit, so a failing assertion never strands the attach.
    struct Release(PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = fs::write(&self.0, "");
        }
    }
    let release = Release(f.root.path().join("query-release"));
    let mut attach = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .args(["--config", f.config.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        attach.stdin.take().unwrap(),
        "{}",
        json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})
    )
    .unwrap();
    wait_until(
        "attach recovery query started",
        Duration::from_secs(5),
        || f.root.path().join("query-started").exists(),
    );
    // The runner observes while attach's recovery query is still in flight.
    let observed = early_observe(&f, "fABC");
    assert_eq!(observed.last().unwrap()["kind"], "result", "{observed:?}");
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
    drop(release);
    let output = attach.wait_with_output().unwrap();
    let frames = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let observation = &frames.last().unwrap()["result"]["observation"];
    assert_eq!(observation["binding"]["id"]["job"], "fABC", "{frames:?}");
    assert_ne!(
        observation["reason"], "recovery transport or identity unavailable",
        "{frames:?}"
    );
    assert_eq!(f.state().binding.unwrap().id.job, "fABC");
    assert_eq!(native_calls(&f.calls(), "jobs -a "), 2, "{}", f.calls());
    assert_eq!(native_calls(&f.calls(), "job submit "), 1, "{}", f.calls());
}

#[test]
#[ignore = "requires /mnt/shared; execute through admitted compute"]
fn x7_plain_slot_matches_consumer_shape() {
    let f = Fixture::new();
    assert_eq!(f.submit()["kind"], "accepted");
    let spec: Value = serde_json::from_slice(
        &fs::read(f.intent.output_dir.join("original-jobspec.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        spec["resources"],
        json!([{"type":"slot","count":1,"label":"task","with":[{"type":"core","count":2},{"type":"memory","count":1}]}])
    );
    let args = fs::read_to_string(f.root.path().join("args")).unwrap();
    assert!(
        !args
            .lines()
            .any(|arg| matches!(arg, "-N" | "-N1" | "-c" | "-g"))
    );
}

#[test]
#[ignore = "requires /mnt/shared; execute through admitted compute"]
fn x7_request_environment_is_closed_and_names_the_refused_key() {
    let mut f = Fixture::new();
    f.request
        .environment
        .insert("UNRECOGNIZED_NAME".into(), "benign".into());
    let frames = f.call(json!({"op":"prepare_intent","request":f.request,"owner":f.intent.owner,"correlation":f.intent.correlation}));
    let frame = frames.last().unwrap();
    assert_eq!(frame["kind"], "error", "{frame}");
    assert!(frame.to_string().contains("UNRECOGNIZED_NAME"), "{frame}");
}

#[test]
#[ignore = "requires /mnt/shared; execute through admitted compute"]
fn x7_flux_uri_is_controller_owned_and_opt_in() {
    for enabled in [false, true] {
        let f = Fixture::new();
        let mut request = serde_json::to_value(&f.request).unwrap();
        request["pass_flux_uri"] = json!(enabled);
        request["repo"] = json!(if enabled {
            "RobTand/prismaquant"
        } else {
            "RobTand/tessera"
        });
        let frames = f.call(json!({"op":"prepare_intent","request":request,"owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_ne!(frames.last().unwrap()["kind"], "error", "{frames:?}");
        let intent = frames.last().unwrap()["result"]["intent"].clone();
        let frames = f.call(json!({"op":"submit","request":request,"intent":intent}));
        assert_eq!(
            frames.last().unwrap()["result"]["submission"]["kind"],
            "accepted",
            "{frames:?}"
        );
        let spec: Value = serde_json::from_slice(
            &fs::read(f.intent.output_dir.join("original-jobspec.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            spec["attributes"]["system"]["environment"].get("FLUX_URI"),
            None,
            "the controller must leave URI injection to the stock job shell"
        );
        let context = f.call(json!({"op":"launch_context", "owner":f.intent.owner,
            "correlation":f.intent.correlation, "job":"fABC"}));
        assert_eq!(
            context.last().unwrap()["result"]["context"]["pass_flux_uri"],
            json!(enabled),
            "{context:?}"
        );
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through admitted compute"]
fn x7_terminal_original_mismatch_retains_scratch_debt() {
    let f = Fixture::new();
    f.submit();
    f.report(ReportOutcome::Passed, true);
    f.events(&completed(0));
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "terminal",
        "{frames:?}"
    );
    let debt = f
        .options
        .journal_root
        .join("cleanup-debt")
        .join(&f.request.runtime.manifest_digest)
        .join(format!("{}.scratch", f.intent.correlation));
    fs::write(&debt, serde_json::to_vec(&f.intent).unwrap()).unwrap();
    foreign_original(&f, "fABC");
    let frames = f.attach();
    assert_eq!(
        frames.last().unwrap()["result"]["observation"]["kind"],
        "unknown",
        "{frames:?}"
    );
    assert!(debt.exists());
}

fn maintenance_backend(f: &Fixture) -> tl3_controller::adapters::run_backend::flux::Flux {
    tl3_controller::adapters::run_backend::flux::Flux::new(f.options.clone()).unwrap()
}

fn maintenance_due(f: &Fixture) -> u64 {
    maintenance_backend(f)
        .next_cleanup_maintenance_ms()
        .unwrap()
        .expect("an attempted, unproven cleanup must schedule durable maintenance")
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_timeout_recovers_with_positive_evidence() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("maintenance-timeout", 'd');
    f.cleanup_control(
        &owned.execution,
        cleanup_limits().native_timeout_seconds + 5,
    );
    let frames = f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    assert_eq!(
        frames.last().unwrap()["result"]["cleanup"]["stopped"],
        false
    );
    assert!(!cleanup_debts(&f).is_empty());
    let due = maintenance_due(&f);
    f.cleanup_control(&owned.execution, 0);
    assert!(
        maintenance_backend(&f)
            .cleanup_maintenance(due)
            .unwrap()
            .is_empty()
    );
    assert!(cleanup_debts(&f).is_empty());
    assert!(!f.state().cleanup_evidence.is_empty());
    assert!(
        maintenance_backend(&f)
            .next_cleanup_maintenance_ms()
            .unwrap()
            .is_none()
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_persistent_failure_never_expires_debt() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("maintenance-failing", 'd');
    f.cleanup_facts(&owned, false);
    f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    for _ in 0..4 {
        let due = maintenance_due(&f);
        let backend = maintenance_backend(&f);
        assert_eq!(
            backend.cleanup_maintenance(due).unwrap(),
            vec![owned.owner.clone()]
        );
        assert!(maintenance_due(&f) > due);
        assert!(!cleanup_debts(&f).is_empty());
        assert!(f.state().cleanup_evidence.is_empty());
    }
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_never_sweeps_healthy_begin_launch_debt() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.ownership();
    let frames = f.call(json!({"op":"begin_container_launch","owner":owned.owner.owner,"correlation":owned.owner.correlation,"launcher":owned}));
    assert_eq!(frames.last().unwrap()["kind"], "result");
    let before = cleanup_debts(&f);
    assert!(!before.is_empty());
    let backend = maintenance_backend(&f);
    assert!(backend.next_cleanup_maintenance_ms().unwrap().is_none());
    assert!(
        backend
            .cleanup_maintenance(u64::MAX - 300_000)
            .unwrap()
            .is_empty()
    );
    assert_eq!(cleanup_debts(&f), before);
    assert!(
        !f.intent
            .output_dir
            .join(format!("helper-called-{}", owned.execution))
            .exists()
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_held_request_dispatches_exactly_once() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("held-debtor", 'd');
    f.cleanup_facts(&owned, false);
    f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    fs::copy(
        f.intent.output_dir.join("original-jobspec.json"),
        f.root.path().join("jobspec-fABC.json"),
    )
    .unwrap();
    let mut intent = f.intent.clone();
    intent.owner = "maintenance-held-owner".into();
    intent.correlation = format!("{}-maintenance-held", f.intent.correlation);
    intent.output_dir = Path::new("/mnt/shared/flux-out/tl3").join(&intent.correlation);
    let backend = maintenance_backend(&f);
    assert!(backend.submit(f.request.clone(), intent.clone()).is_err());
    let state_path = f
        .options
        .journal_root
        .join(&intent.correlation)
        .join("state.json");
    let state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(state["dispatched"], false);
    assert!(
        state["held_submission_ms"].is_u64(),
        "gate-held intent must be explicitly resumable: {state}"
    );
    let due = maintenance_due(&f).max(state["held_submission_ms"].as_u64().unwrap());
    f.cleanup_facts(&owned, true);
    fs::write(f.root.path().join("submit-id"), "fDEF\n").unwrap();
    assert!(backend.cleanup_maintenance(due).unwrap().is_empty());
    backend.cleanup_maintenance(due + 300_000).unwrap();
    // Maintenance alone must dispatch; an explicit submit would hide a broken resume loop.
    let state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(state["dispatched"], true);
    assert!(state["held_submission_ms"].is_null());
    let calls = fs::read_to_string(f.root.path().join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("job submit "))
            .count(),
        2,
        "one original, one resumed: {calls}"
    );
    fs::remove_dir_all(&intent.output_dir).unwrap();
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_never_resubmits_possible_native_acceptance() {
    let f = Fixture::new();
    fs::write(f.root.path().join("mode"), "unknown").unwrap();
    assert_eq!(f.submit()["kind"], "unknown");
    assert!(f.state().dispatched);
    let backend = maintenance_backend(&f);
    assert!(backend.next_cleanup_maintenance_ms().unwrap().is_none());
    backend.cleanup_maintenance(u64::MAX - 300_000).unwrap();
    assert!(matches!(
        backend.submit(f.request.clone(), f.intent.clone()).unwrap(),
        Submission::Unknown { .. }
    ));
    let calls = fs::read_to_string(f.root.path().join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("job submit "))
            .count(),
        1,
        "{calls}"
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_restart_recovers_durable_attempt_and_serializes_owner() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let first = f.cleanup_execution("restart-first", 'd');
    let second = f.cleanup_execution("restart-second", 'e');
    f.cleanup_facts(&first, false);
    f.cleanup_facts(&second, false);
    f.call(json!({"op":"cleanup_runtime","owner":first.owner.owner,"correlation":first.owner.correlation}));
    let due = maintenance_due(&f);
    // The first pass crashes after advancing its durable claim. A fresh backend
    // must retain that deadline and retry every remaining execution when due.
    let path = f
        .options
        .journal_root
        .join(&f.intent.correlation)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["cleanup_retry_ms"] = json!(due + 300_000);
    api::durable::atomic(&path, &serde_json::to_vec(&state).unwrap()).unwrap();
    f.cleanup_facts(&first, true);
    f.cleanup_facts(&second, true);
    f.cleanup_control(&first.execution, 1);
    fs::remove_file(
        f.intent
            .output_dir
            .join(format!("helper-called-{}", first.execution)),
    )
    .unwrap();
    let backend = maintenance_backend(&f);
    assert_eq!(
        backend.next_cleanup_maintenance_ms().unwrap(),
        Some(due + 300_000)
    );
    backend.cleanup_maintenance(due).unwrap();
    assert!(
        !f.intent
            .output_dir
            .join(format!("helper-called-{}", first.execution))
            .exists()
    );
    let one = backend.clone();
    let two = backend.clone();
    let worker = std::thread::spawn(move || one.cleanup_maintenance(due + 300_000).unwrap());
    wait_until("maintenance helper started", Duration::from_secs(5), || {
        f.intent
            .output_dir
            .join(format!("helper-called-{}", first.execution))
            .exists()
    });
    two.cleanup_maintenance(due + 300_000).unwrap();
    worker.join().unwrap();
    assert!(cleanup_debts(&f).is_empty());
    assert_eq!(f.state().cleanup_evidence.len(), 2);
    for owned in [&first, &second] {
        let attempts = fs::read_to_string(
            f.intent
                .output_dir
                .join(format!("helper-attempts-{}", owned.execution)),
        )
        .unwrap();
        assert_eq!(
            attempts.lines().count(),
            2,
            "one failed attempt and one retry per execution"
        );
    }
    assert!(
        maintenance_backend(&f)
            .next_cleanup_maintenance_ms()
            .unwrap()
            .is_none()
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_corrupt_owner_does_not_block_healthy_owner_or_spin() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("isolated-owner", 'd');
    f.cleanup_facts(&owned, false);
    f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    let due = maintenance_due(&f);
    let corrupt = f.options.journal_root.join("corrupt-owner");
    fs::create_dir(&corrupt).unwrap();
    fs::write(corrupt.join("state.json"), b"{truncated").unwrap();
    f.cleanup_facts(&owned, true);
    let backend = maintenance_backend(&f);
    // Exercise the real Look entry point with the corrupt and healthy backend
    // records still present together, before any direct maintenance call.
    let mut database = maintenance_store_support::Database::start();
    struct Clock(u64);
    impl tl3_controller::clock::Clock for Clock {
        fn now(&self) -> std::time::SystemTime {
            std::time::UNIX_EPOCH + Duration::from_millis(self.0)
        }
    }
    struct Maintenance(tl3_controller::adapters::run_backend::flux::Flux);
    impl tl3_controller::executor::Executor for Maintenance {
        fn execute(
            &mut self,
            _: &tl3_controller::executor::Delivery,
        ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
            unreachable!()
        }
        fn reconcile(
            &mut self,
            _: &tl3_controller::executor::Delivery,
        ) -> tl3_controller::Result<tl3_controller::executor::Reconciliation> {
            unreachable!()
        }
        fn cleanup_maintenance(&mut self, now: u64) -> tl3_controller::Result<Vec<String>> {
            self.0
                .cleanup_maintenance(now)
                .map(|owners| {
                    owners
                        .into_iter()
                        .map(|owner| format!("{}:{}", owner.owner, owner.correlation))
                        .collect()
                })
                .map_err(|error| tl3_controller::Error::Adapter(Box::new(error)))
        }
        fn next_cleanup_maintenance_ms(&mut self) -> tl3_controller::Result<Option<u64>> {
            self.0
                .next_cleanup_maintenance_ms()
                .map_err(|error| tl3_controller::Error::Adapter(Box::new(error)))
        }
    }
    let mut registry = tl3_controller::adapters::simulation(f.root.path()).unwrap();
    registry.register(
        tl3_controller::executor::Upstream::PrismaBuild,
        Maintenance(backend.clone()),
    );
    let mut controller = tl3_controller::Controller::open_with_clock(
        tl3_store::Store::open(&database.create("maintenance_corrupt_owner")).unwrap(),
        registry,
        tl3_controller::Bootstrap {
            repos: vec![],
            rob: 42,
            slots: 1,
            pool: 1,
            fail_fast: false,
            operations: None,
        },
        Clock(due),
    )
    .unwrap();
    controller.look_now().unwrap();
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "incidents"
                && row.value["open"] == true
                && row.value["signature"]["class"] == "runtime_cleanup")
    );
    assert!(
        controller
            .next_look_in()
            .unwrap()
            .is_some_and(|delay| delay > Duration::ZERO)
    );
    drop(controller);
    database.stop();
    let failures = backend.cleanup_maintenance(due).unwrap();
    assert!(
        !failures.is_empty(),
        "corrupt owner must be reported for an incident"
    );
    assert!(
        cleanup_debts(&f).is_empty(),
        "healthy owner must still be maintained"
    );
    assert!(!f.state().cleanup_evidence.is_empty());
    assert!(
        backend
            .next_cleanup_maintenance_ms()
            .unwrap()
            .is_some_and(|next| next > due),
        "bad owner must back off instead of keeping the timer overdue"
    );
    let errors = f.options.journal_root.join("maintenance-errors");
    assert!(
        errors.read_dir().unwrap().next().is_some(),
        "owner errors must survive restart"
    );
    let restarted = maintenance_backend(&f);
    assert!(
        restarted
            .next_cleanup_maintenance_ms()
            .unwrap()
            .is_some_and(|next| next > due)
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_initial_attempt_is_durable_before_helper_returns() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("initial-claim", 'd');
    f.cleanup_control(&owned.execution, 1);
    let (mut child, receive, reader) = f.spawn_operation("cleanup_runtime", 0);
    recv_within("initial cleanup progress", &receive, Duration::from_secs(5));
    wait_until("initial helper started", Duration::from_secs(5), || {
        f.intent
            .output_dir
            .join("helper-called-initial-claim")
            .exists()
    });
    let eligible = f.state().cleanup_retry_ms;
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    assert!(
        eligible.is_some(),
        "a crash during helper I/O must leave durable retry eligibility"
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_contended_request_is_durable_without_a_helper() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("contended", 'd');
    let backend = maintenance_backend(&f);
    let journal = api::durable::Journal::new(f.options.journal_root.clone()).unwrap();
    let lock = journal
        .operation_lock(&owned.owner, "cleanup-attempt-lock")
        .unwrap();
    lock.lock().unwrap();
    let result = backend.cleanup_runtime(&owned.owner).unwrap();
    assert!(!result.stopped);
    assert!(
        f.state().cleanup_retry_ms.is_some(),
        "an in-flight lock must not discard a new cleanup request"
    );
    assert!(!f.intent.output_dir.join("helper-called-contended").exists());
    drop(lock);
    backend.cleanup_maintenance(maintenance_due(&f)).unwrap();
    assert!(cleanup_debts(&f).is_empty());
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_busy_owner_is_skipped_and_remains_due() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("busy-owner", 'd');
    f.cleanup_facts(&owned, false);
    f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    let due = maintenance_due(&f);
    f.cleanup_facts(&owned, true);
    fs::remove_file(f.intent.output_dir.join("helper-called-busy-owner")).unwrap();
    let lock = fs::OpenOptions::new().read(true).write(true)
        .open(f.options.journal_root.join(&f.intent.correlation).join("lock")).unwrap();
    lock.lock().unwrap();
    let backend = maintenance_backend(&f);
    let pending = backend.cleanup_maintenance(due).unwrap();
    let deadline = backend.next_cleanup_maintenance_ms().unwrap();
    assert!(pending.is_empty(), "contention must not report an incident owner");
    assert!(api::durable::maintenance_failures(&f.options.journal_root).unwrap().is_empty(),
        "contention must not count as a maintenance failure, including during timer scans");
    assert!(deadline.is_some_and(|next| next <= due), "busy owners need an early retry");
    assert_eq!(f.state().cleanup_retry_ms, Some(due), "a skipped owner must not be claimed");
    assert!(!f.intent.output_dir.join("helper-called-busy-owner").exists());
    drop(lock);
    assert_eq!(backend.next_cleanup_maintenance_ms().unwrap(), Some(due));
    backend.cleanup_maintenance(due).unwrap();
    assert!(cleanup_debts(&f).is_empty(), "the very next pass must retry the owner");
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_owner_limit_leaves_remaining_owners_due() {
    let mut fixtures = Vec::new();
    for name in ["bounded-first", "bounded-second"] {
        let mut f = Fixture::new();
        f.podman(ContainerMode::OwnScope, true);
        f.submit();
        let owned = f.cleanup_execution(name, 'd');
        f.cleanup_facts(&owned, false);
        f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
        fs::remove_file(f.intent.output_dir.join(format!("helper-called-{name}"))).unwrap();
        fixtures.push(f);
    }
    let first = &fixtures[0];
    let second = &fixtures[1];
    let dir = first.options.journal_root.join(&second.intent.correlation);
    fs::create_dir(&dir).unwrap();
    fs::copy(
        second
            .options
            .journal_root
            .join(&second.intent.correlation)
            .join("state.json"),
        dir.join("state.json"),
    )
    .unwrap();
    let due = fixtures.iter().map(maintenance_due).max().unwrap();
    let backend =
        maintenance_backend(first).with_cleanup_maintenance_limits(1, Duration::from_secs(5));
    backend.cleanup_maintenance(due).unwrap();
    assert_eq!(
        fixtures
            .iter()
            .filter(
                |f| f.intent.output_dir.read_dir().unwrap().any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("helper-called-"))
            )
            .count(),
        1,
        "one owner per pass"
    );
    assert!(
        backend
            .next_cleanup_maintenance_ms()
            .unwrap()
            .is_some_and(|next| next <= due),
        "unattempted owners remain due"
    );
    backend.cleanup_maintenance(due).unwrap();
    assert_eq!(
        fixtures
            .iter()
            .filter(
                |f| f.intent.output_dir.read_dir().unwrap().any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("helper-called-"))
            )
            .count(),
        2,
        "next pass reaches remaining owner"
    );
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_time_budget_stops_between_executions() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let first = f.cleanup_execution("budget-first", 'd');
    let second = f.cleanup_execution("budget-second", 'e');
    f.cleanup_facts(&first, false);
    f.cleanup_facts(&second, false);
    f.call(json!({"op":"cleanup_runtime","owner":first.owner.owner,"correlation":first.owner.correlation}));
    let mut later = Fixture::new();
    later.podman(ContainerMode::OwnScope, true);
    later.submit();
    let later_owned = later.cleanup_execution("budget-later-owner", 'd');
    later.cleanup_facts(&later_owned, false);
    later.call(json!({"op":"cleanup_runtime","owner":later_owned.owner.owner,"correlation":later_owned.owner.correlation}));
    // Sort this owner after the two-execution owner in the shared journal.
    let mut state = later.state();
    state.intent.correlation = format!("{}-later", f.intent.correlation);
    let dir = f.options.journal_root.join(&state.intent.correlation);
    fs::create_dir(&dir).unwrap();
    api::durable::save(&dir, &state).unwrap();
    fs::remove_file(later.intent.output_dir.join("helper-called-budget-later-owner")).unwrap();
    for owned in [&first, &second] {
        fs::remove_file(
            f.intent.output_dir.join(format!("helper-called-{}", owned.execution)),
        ).unwrap();
        f.cleanup_facts(owned, true);
    }
    f.cleanup_control(&first.execution, 2);
    let before = cleanup_debts(&f);
    assert_eq!(before.len(), 2);
    let due = maintenance_due(&f).max(state.cleanup_retry_ms.unwrap());
    let backend =
        maintenance_backend(&f).with_cleanup_maintenance_limits(4, Duration::from_millis(500));
    let started = Instant::now();
    backend.cleanup_maintenance(due).unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2),
        "a started helper keeps its own budget even after the pass budget expires");
    assert!(f.state().cleanup_evidence.contains_key(&first.execution),
        "the running helper must finish and retain positive stop evidence");
    assert_eq!(cleanup_debts(&f).len(), 1,
        "the completed execution must retire its own debt");
    assert!(!f.state().cleanup_evidence.contains_key(&second.execution));
    assert!(!f.intent.output_dir.join("helper-called-budget-second").exists(),
        "budget exhaustion must not start another execution");
    assert!(!later.intent.output_dir.join("helper-called-budget-later-owner").exists(),
        "budget exhaustion must not start another owner");
    let untouched = serde_json::from_slice::<api::durable::Record>(
        &fs::read(dir.join("state.json")).unwrap()).unwrap();
    assert_eq!(untouched.cleanup_retry_ms, state.cleanup_retry_ms,
        "an unstarted owner must remain due without a claim");
    assert!(backend.next_cleanup_maintenance_ms().unwrap().is_some_and(|next| next <= due));
}

fn hold_maintenance_request(f: &mut Fixture) -> SubmissionIntent {
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("held-maintenance", 'd');
    f.cleanup_facts(&owned, false);
    f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    fs::copy(
        f.intent.output_dir.join("original-jobspec.json"),
        f.root.path().join("jobspec-fABC.json"),
    )
    .unwrap();
    let mut intent = f.intent.clone();
    intent.owner = "held-maintenance-owner".into();
    intent.correlation = format!("{}-held", f.intent.correlation);
    intent.output_dir = Path::new("/mnt/shared/flux-out/tl3").join(&intent.correlation);
    assert!(
        maintenance_backend(f)
            .submit(f.request.clone(), intent.clone())
            .is_err()
    );
    f.cleanup_facts(&owned, true);
    intent
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_failed_held_request_is_reported_and_durable() {
    let mut f = Fixture::new();
    let intent = hold_maintenance_request(&mut f);
    let path = f
        .options
        .journal_root
        .join(&intent.correlation)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let spec = PathBuf::from(state["jobspec"]["path"].as_str().unwrap());
    fs::write(spec, b"changed staged jobspec").unwrap();
    state["held_submission_ms"] = json!(0);
    api::durable::atomic(&path, &serde_json::to_vec(&state).unwrap()).unwrap();
    let backend = maintenance_backend(&f);
    let due = maintenance_due(&f);
    let pending = backend.cleanup_maintenance(due).unwrap();
    assert!(
        pending
            .iter()
            .any(|owner| owner.correlation == intent.correlation),
        "unresumable request must raise an incident"
    );
    assert!(
        backend
            .next_cleanup_maintenance_ms()
            .unwrap()
            .is_some_and(|next| next > due)
    );
    assert!(
        f.options
            .journal_root
            .join("maintenance-errors")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
    let state: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(state["dispatched"], false);
    fs::remove_dir_all(&intent.output_dir).unwrap();
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_held_dispatch_requires_upstream_authorization() {
    let mut f = Fixture::new();
    let intent = hold_maintenance_request(&mut f);
    let due = maintenance_due(&f) + 300_000;
    let calls = std::cell::Cell::new(0);
    let authorize = |owner: &Owner| {
        assert_eq!(owner.correlation, intent.correlation);
        calls.set(calls.get() + 1);
        Err(BackendError::invalid("fixture epoch superseded"))
    };
    let backend = maintenance_backend(&f);
    let pending = backend.cleanup_maintenance_fenced(due, &authorize).unwrap();
    assert_eq!(
        calls.get(),
        1,
        "resuming a held request must visit the upstream fence"
    );
    assert!(
        pending
            .iter()
            .any(|owner| owner.correlation == intent.correlation)
    );
    let state: Value = serde_json::from_slice(
        &fs::read(
            f.options
                .journal_root
                .join(&intent.correlation)
                .join("state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(state["dispatched"], false);
    assert_eq!(
        fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("job submit "))
            .count(),
        1
    );
    fs::remove_dir_all(&intent.output_dir).unwrap();
}

#[test]
#[ignore = "requires /mnt/shared; run through admitted fleet compute"]
fn cleanup_maintenance_terminal_debt_is_loud_without_a_noop_timer() {
    let mut f = Fixture::new();
    f.podman(ContainerMode::OwnScope, true);
    f.submit();
    let owned = f.cleanup_execution("terminal-debt", 'd');
    f.cleanup_facts(&owned, false);
    f.call(json!({"op":"cleanup_runtime","owner":owned.owner.owner,"correlation":owned.owner.correlation}));
    let due = maintenance_due(&f);
    let path = f
        .options
        .journal_root
        .join(&f.intent.correlation)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["terminal"] = json!(Terminal {
        ending: Ending::InfraLost,
        reason: "fixture committed infrastructure loss with unresolved cleanup".into(),
        receipt: BlobRef {
            path: f.intent.output_dir.join("terminal-receipt.json"),
            sha256: "a".repeat(64),
            bytes: 1
        },
    });
    api::durable::atomic(&path, &serde_json::to_vec(&state).unwrap()).unwrap();
    let backend = maintenance_backend(&f);
    assert_eq!(
        backend.cleanup_maintenance(due).unwrap(),
        vec![owned.owner.clone()]
    );
    assert!(
        backend.next_cleanup_maintenance_ms().unwrap().is_none(),
        "immutable terminal cleanup cannot make progress through retries"
    );
    let failures = f
        .options
        .journal_root
        .join("maintenance-errors")
        .read_dir()
        .unwrap()
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1);
    let failure: Value =
        serde_json::from_slice(&fs::read(failures[0].as_ref().unwrap().path()).unwrap()).unwrap();
    assert!(
        failure["message"]
            .as_str()
            .unwrap()
            .contains("terminal cleanup is immutable")
    );
    assert!(!cleanup_debts(&f).is_empty());
}
