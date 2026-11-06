use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::path::{Path, PathBuf};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tl3_controller::run_backend::{self as api, durable::digest, *};

fn cli(request: &Value) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn cli_schema_is_generated_from_rust_and_describes_all_operations() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend"));
    let output = command.arg("schema").output().unwrap();
    let schema: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = schema.to_string();
    for operation in ["submit", "attach", "cancel", "probe"] {
        assert!(text.contains(operation), "missing operation {operation}");
    }
    assert!(text.contains("LaunchContext"));
}

#[test]
fn malformed_requests_have_a_versioned_bounded_error_frame() {
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
}

fn immutable(path: &Path, bytes: &[u8], executable: bool) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 })).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let shared = Path::new("/mnt/shared/flux-out/tl3");
        fs::create_dir_all(shared).unwrap();
        let root = tempfile::Builder::new().prefix("fx1-fixture-").tempdir_in(shared).unwrap();
        let p = root.path();
        let correlation = format!("{}-out", p.file_name().unwrap().to_str().unwrap());
        let tree = p.join("tree");
        fs::create_dir(&tree).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin")
                .arg("-C").arg(&tree).args(args).output().unwrap();
            assert!(output.status.success(), "git fixture setup failed");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--quiet"]);
        fs::write(tree.join("subject.txt"), b"immutable subject\n").unwrap();
        git(&["add", "subject.txt"]);
        git(&["-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "fixture"]);
        let head = git(&["rev-parse", "HEAD"]);
        let tree_id = git(&["rev-parse", "HEAD^{tree}"]);
        immutable(&tree.join("subject.txt"), b"immutable subject\n", false);
        let input = serde_json::to_vec(&InputManifest { schema: "tl3.input_manifest.v1".into(), head: head.clone(), tree: tree_id.clone(),
            files: vec![InputFile { path: "subject.txt".into(), sha256: digest(b"immutable subject\n") }] }).unwrap();
        immutable(&p.join("input.json"), &input, false);
        immutable(&p.join("runner"), b"#!/bin/sh\nexit 0\n", true);
        immutable(&p.join("runner.json"), b"{\"runner\":1}", false);
        immutable(&p.join("runtime.json"), b"{\"runtime\":1}", false);
        immutable(&p.join("preflight.json"), &serde_json::to_vec(&RuntimePreflight { schema: "tl3.runtime_preflight.v1".into(), architecture: "x86_64".into(), image_identity: None, image_present: false, identity_verified: true, image: None, mode: None, no_cgroups_supported: false, init_available: false, user_scope_supported: false }).unwrap(), false);
        let preflight = fs::read(p.join("preflight.json")).unwrap();
        let membership = serde_json::to_vec(&Membership { schema: "tl3.membership.v1".into(), cases: vec![CaseIdentity { target: "unit".into(), name: "case".into() }] }).unwrap();
        let request = Request {
            repo: "fixture/repo".into(),
            tree: TreeIdentity { head, tree: tree_id, input_digest: digest(&input), root: tree, manifest: p.join("input.json") },
            suite: Suite::SelectedCases { membership_digest: digest(&membership) },
            reservation: Reservation { cpus: 2.try_into().unwrap(), gpus: 0, memory_bytes: (1024 * 1024 * 1024).try_into().unwrap(),
                memory_guarantee: MemoryGuarantee::DeclaredOnly, parallelism: 2.try_into().unwrap(), native_threads: 1.try_into().unwrap() },
            runner: RunnerIdentity { executable: p.join("runner"), executable_digest: digest(b"#!/bin/sh\nexit 0\n"), arguments: vec!["--fixture".into()],
                manifest: p.join("runner.json"), manifest_digest: digest(b"{\"runner\":1}") },
            environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            runtime: RuntimeIdentity { manifest: p.join("runtime.json"), manifest_digest: digest(b"{\"runtime\":1}"),
                runtime: ExecutionRuntime::Native { interpreter: "/usr/bin/python3".into() }, architecture: "x86_64".into(), expected_image: None,
                preflight: BlobRef { sha256: digest(&preflight), bytes: preflight.len() as u64, path: p.join("preflight.json") },
                host_dependency: Some(HostDependency { host: "dl380g10".into(), reason: "fixture of the provisioned amd64 image dependency".into() }), image: None, cleanup: None },
            scratch_root: "/var/tmp/flux-vanilla".into(), no_progress_seconds: None, walltime_seconds: 60.try_into().unwrap(),
        };
        let intent = SubmissionIntent { backend: BackendTag::Flux, instance: "fixture-generation".into(), uri: "local:///fixture".into(), owner: "fixture-owner".into(),
            correlation: correlation.clone(), request_digest: digest(&serde_json::to_vec(&request).unwrap()), output_dir: shared.join(&correlation) };
        let config = p.join("options.json");
        let native = p.join("native-flux");
        fs::write(&native, r#"#!/bin/sh
set -eu
fixture=$1
shift
printf '%s\n' "$*" >> "$fixture/calls"
mode=$(cat "$fixture/mode")
case "$1" in
submit)
    printf '%s\n' "$@" > "$fixture/args"
    /usr/bin/python3 "$fixture/jobspec.py" "$fixture" "$@";;
jobs) if [ "$mode" = quiet-jobs ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi; cat "$fixture/jobs.json";;
cancel) printf 'acknowledged\n';;
resource)
    case "$3" in --states=all) cat "$fixture/all.json";; --states=free) cat "$fixture/free.json";; *) exit 9;; esac;;
job)
    case "$2" in
    submit) if [ "$mode" = unknown ]; then exit 1; fi; printf 'fABC\n';;
    info)
        if [ "$mode" = quiet-info ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi
        if [ "$3" = --original ]; then cat "$fixture/jobspec.json"; else cat "$fixture/all.json"; fi;;
    eventlog)
        if [ "$mode" = quiet ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi
        if [ "$mode" = interrupted ]; then head -n 1 "$fixture/events"; exit 1; fi
        cat "$fixture/events";;
    status) if [ "$mode" = quiet-collect ]; then touch "$fixture/watching"; exec /bin/sleep 300; fi; printf '{"result":1}\n';;
    *) exit 9;; esac;;
*) exit 9;; esac
"#).unwrap();
        fs::write(p.join("jobspec.py"), r#"import json,sys,pathlib
root=pathlib.Path(sys.argv[1]);args=sys.argv[2:]
def option(prefix): return next(item[len(prefix):] for item in args if item.startswith(prefix))
environment=dict(item[6:].split('=',1) for item in args if item.startswith('--env=') and '=' in item[6:])
user=dict(item[15:].split('=',1) for item in args if item.startswith('--setattr=user.'))
runner=next(i for i,item in enumerate(args) if not item.startswith('-') and item.startswith('/mnt/shared/'))
spec={'tasks':[{'command':args[runner:]}], 'resources':[{'type':'core','count':int(args[args.index('-c')+1])}], 'attributes':{'user':user,'system':{'duration':option('-t') if any(item.startswith('-t=') for item in args) else args[args.index('-t')+1], 'constraints':option('--requires='), 'cwd':option('--cwd='),'environment':environment,'job':{'name':option('--job-name=')}, 'shell':{'options':{'output':{'stdout':{'path':option('--output=')},'stderr':{'path':option('--error=')}}}}}}}
text=json.dumps(spec);(root/'jobspec.json').write_text(text);print(text)
"#).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o555)).unwrap();
        fs::write(p.join("mode"), "normal").unwrap();
        let all = json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":"0-3"}}],"nodelist":["fixture-host"]}});
        fs::write(p.join("all.json"), all.to_string()).unwrap();
        fs::write(p.join("free.json"), all.to_string()).unwrap();
        fs::write(p.join("jobs.json"), json!({"jobs":[{"name":correlation,"jobid":"fABC","id":42}]}).to_string()).unwrap();
        let options = Options { executable: native, arguments: vec![p.to_string_lossy().into_owned()], environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            instance_generation: "fixture-generation".into(), uri: "local:///fixture".into(), journal_root: p.join("journal"),
            bootstrap: HostBootstrap { cli: env!("CARGO_BIN_EXE_tl3-run-backend").into(), config: config.clone() } };
        fs::write(&config, serde_json::to_vec(&options).unwrap()).unwrap();
        Self { root, request, intent, options, config, membership }
    }

    fn call(&self, command: Value) -> Vec<Value> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend")).args(["--config", self.config.to_str().unwrap()])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        writeln!(child.stdin.take().unwrap(), "{command}").unwrap();
        let output = child.wait_with_output().unwrap();
        String::from_utf8(output.stdout).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect()
    }
    fn submit(&self) -> Value {
        let frames = self.call(json!({"op":"submit","request":self.request,"intent":self.intent}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        frames.last().unwrap()["result"]["submission"].clone()
    }
    fn ownership(&self) -> RuntimeOwnership {
        RuntimeOwnership { run: BackendId { backend: BackendTag::Flux, instance: self.intent.instance.clone(), uri: self.intent.uri.clone(), job: "fABC".into() },
            owner: Owner { owner: self.intent.owner.clone(), correlation: self.intent.correlation.clone() }, request_digest: self.intent.request_digest.clone(),
            host: "dl380g10".into(), rank: 0, launcher: ProcessIdentity { pid: 1234, start_ticks: 5678 }, process_group: 1234, container: None }
    }
    fn report(&self, outcome: ReportOutcome, cleanup_complete: bool) {
        let out = &self.intent.output_dir;
        fs::write(out.join("membership.json"), &self.membership).unwrap();
        let owned = self.ownership();
        let frames = self.call(json!({"op":"retain_runtime","owner":self.intent.owner,"correlation":self.intent.correlation,"ownership":owned}));
        assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
        let cleanup_bytes = serde_json::to_vec(&CleanupEvidence { schema: "tl3.cleanup_evidence.v1".into(), ownership: owned,
            launcher: ProcessStop::Reaped { status: 0 }, process_group_probe_errno: Some(3), container: None }).unwrap();
        fs::write(out.join("cleanup.txt"), &cleanup_bytes).unwrap();
        if cleanup_complete {
            let frames = self.call(json!({"op":"retain_cleanup","owner":self.intent.owner,"correlation":self.intent.correlation,
                "evidence":BlobRef { sha256: digest(&cleanup_bytes), bytes: cleanup_bytes.len() as u64, path: out.join("cleanup.txt") }}));
            assert_eq!(frames.last().unwrap()["result"]["cleanup"]["stopped"], true, "{frames:?}");
        }
        let report = RunnerReport {
            schema: "tl3.runner_report.v1".into(), run: BackendId { backend: BackendTag::Flux, instance: self.intent.instance.clone(), uri: self.intent.uri.clone(), job: "fABC".into() },
            request_digest: self.intent.request_digest.clone(), tree: self.request.tree.clone(), runner_digest: self.request.runner.executable_digest.clone(), runtime_digest: self.request.runtime.manifest_digest.clone(),
            suite: self.request.suite.clone(), complete: true, cleanup_complete, tests_started: 1, outcome,
            cases: vec![CaseFact { target: "unit".into(), name: "case".into(), outcome: if outcome == ReportOutcome::TestsFailed { CaseOutcome::Failed } else { CaseOutcome::Passed }, duration_ms: 1, disposition: None }],
            payload: None, runtime_binding: None, cleanup: Some(BlobRef { sha256: digest(&cleanup_bytes), bytes: cleanup_bytes.len() as u64, path: out.join("cleanup.txt") }), stdout: None,
        };
        fs::write(out.join("report.json"), serde_json::to_vec(&report).unwrap()).unwrap();
    }
    fn events(&self, events: &[Value]) {
        let text = events.iter().map(|event| format!("{event}\n")).collect::<String>();
        fs::write(self.root.path().join("events"), text).unwrap();
    }
    fn attach(&self) -> Vec<Value> { self.call(json!({"op":"attach","owner":self.intent.owner,"correlation":self.intent.correlation})) }
    fn calls(&self) -> String { fs::read_to_string(self.root.path().join("calls")).unwrap_or_default() }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.intent.output_dir); }
}

fn completed(status: u32) -> Vec<Value> {
    vec![json!({"timestamp":1.0,"name":"start","context":{}}), json!({"timestamp":2.0,"name":"finish","context":{"status":status}}), json!({"timestamp":3.0,"name":"clean","context":{}})]
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn acceptance_unknown_never_resubmits_and_original_jobspec_recovers_one_match() {
    let f = Fixture::new();
    fs::write(f.root.path().join("mode"), "unknown").unwrap();
    assert_eq!(f.submit()["kind"], "unknown");
    assert_eq!(f.submit()["kind"], "unknown");
    assert_eq!(f.calls().lines().filter(|line| line.starts_with("job submit ")).count(), 1);
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    f.report(ReportOutcome::Passed, true);
    f.events(&completed(0));
    let frames = f.attach();
    assert_eq!(frames.last().unwrap()["result"]["observation"]["terminal"]["ending"], "passed", "{frames:?}");
    assert!(f.calls().contains("job info --original fABC jobspec"));
    let state: Value = serde_json::from_slice(&fs::read(f.options.journal_root.join(&f.intent.correlation).join("state.json")).unwrap()).unwrap();
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
        assert_eq!(f.attach().last().unwrap()["result"]["observation"]["kind"], "unknown");
        assert_eq!(f.calls().lines().filter(|line| line.starts_with("job submit ")).count(), 1);
    }
    let f = Fixture::new();
    fs::write(f.root.path().join("mode"), "unknown").unwrap();
    f.submit();
    let path = f.root.path().join("jobspec.json");
    let mut spec: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    spec["attributes"]["user"]["tree_digest"] = json!("different-tree");
    fs::write(path, spec.to_string()).unwrap();
    assert_eq!(f.attach().last().unwrap()["result"]["observation"]["kind"], "unknown");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn replay_deduplicates_retained_prefix_and_transport_error_stays_unknown() {
    let f = Fixture::new();
    f.submit(); f.report(ReportOutcome::Passed, true); f.events(&completed(0));
    fs::write(f.root.path().join("mode"), "interrupted").unwrap();
    let first = f.attach();
    assert_eq!(first.last().unwrap()["result"]["observation"]["kind"], "unknown");
    assert_eq!(first.iter().filter(|frame| frame["kind"] == "event").count(), 1);
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    let second = f.attach();
    assert_eq!(second.iter().filter(|frame| frame["kind"] == "event").count(), 2);
    assert_eq!(second.last().unwrap()["result"]["observation"]["terminal"]["ending"], "passed", "{second:?}");
    assert_eq!(f.attach().iter().filter(|frame| frame["kind"] == "event").count(), 0);
    assert!(!f.calls().contains("job wait "));
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn posix_and_five_endings_require_semantic_and_cleanup_evidence() {
    assert_eq!(api::decode_wait_status(256), WaitStatus::Exited { raw: 256, code: 1 });
    assert_eq!(api::decode_wait_status(15), WaitStatus::Signalled { raw: 15, signal: 15, core_dumped: false });
    assert_eq!(api::decode_wait_status(139), WaitStatus::Signalled { raw: 139, signal: 11, core_dumped: true });
    for (outcome, status, expected) in [(ReportOutcome::Passed, 0, "passed"), (ReportOutcome::TestsFailed, 256, "failed")] {
        let f = Fixture::new(); f.submit(); f.report(outcome, true); f.events(&completed(status));
        let frames = f.attach();
        assert_eq!(frames.last().unwrap()["result"]["observation"]["terminal"]["ending"], expected, "{frames:?}");
        let receipt: BlobRef = serde_json::from_value(frames.last().unwrap()["result"]["observation"]["terminal"]["receipt"].clone()).unwrap();
        let bytes = fs::read(&receipt.path).unwrap();
        assert_eq!(digest(&bytes), receipt.sha256);
        let receipt: Receipt = serde_json::from_slice(&bytes).unwrap();
        assert!(receipt.evidence.iter().any(|blob| blob.path.ends_with("eventlog.jsonl")));
        for reference in receipt.evidence { assert_eq!(digest(&fs::read(reference.path).unwrap()), reference.sha256); }
    }
    let f = Fixture::new(); f.submit(); f.events(&completed(0));
    assert_eq!(f.attach().last().unwrap()["result"]["observation"]["kind"], "unknown");
    let f = Fixture::new(); f.submit(); f.report(ReportOutcome::TestsFailed, true); f.events(&completed(15));
    assert_eq!(f.attach().last().unwrap()["result"]["observation"]["kind"], "unknown");
    let mut f = Fixture::new();
    f.request.reservation.memory_guarantee = MemoryGuarantee::RequireAdmission;
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap());
    assert_eq!(f.submit()["terminal"]["ending"], "refused");
    assert!(!f.calls().contains("job submit "));
    let f = Fixture::new(); f.submit(); f.report(ReportOutcome::EnvironmentLost, true);
    f.events(&[json!({"name":"exception","context":{"type":"exec","severity":0,"note":"owned execution lost"}}), json!({"name":"clean"})]);
    assert_eq!(f.attach().last().unwrap()["result"]["observation"]["terminal"]["ending"], "infra-lost");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cancel_ack_needs_native_cancel_and_stopped_scope_and_finish_race_keeps_original() {
    for finish_first in [false, true] {
        let f = Fixture::new(); f.submit(); f.report(ReportOutcome::Passed, true);
        let ack = f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
        assert_eq!(ack.last().unwrap()["result"]["acknowledgement"]["acknowledged"], true);
        assert_eq!(ack.last().unwrap()["result"]["acknowledgement"]["terminal"], Value::Null);
        let cancel = json!({"name":"exception","context":{"type":"cancel","severity":0,"note":"owned cancellation"}});
        let mut events = completed(0);
        events.insert(if finish_first { 2 } else { 1 }, cancel);
        f.events(&events);
        assert_eq!(f.attach().last().unwrap()["result"]["observation"]["terminal"]["ending"], if finish_first { "passed" } else { "cancelled" });
    }
    let f = Fixture::new(); f.submit(); f.report(ReportOutcome::Passed, false);
    f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    f.events(&[json!({"name":"exception","context":{"type":"cancel","severity":0}}), json!({"name":"clean"})]);
    assert_eq!(f.attach().last().unwrap()["result"]["observation"]["kind"], "unknown");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn probe_reports_busy_incompatible_unknown_and_never_ram_admission() {
    let mut f = Fixture::new();
    let placement = f.request.runtime.placement();
    let command = |r: &Reservation| json!({"op":"probe","reservation":r,"placement":placement});
    assert_eq!(f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"], "available");
    fs::write(f.root.path().join("free.json"), json!({"version":1,"execution":{"R_lite":[]}}).to_string()).unwrap();
    assert_eq!(f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"], "busy");
    f.request.reservation.cpus = 8.try_into().unwrap();
    assert_eq!(f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"], "incompatible");
    fs::write(f.root.path().join("all.json"), "bad-json").unwrap();
    assert_eq!(f.call(command(&f.request.reservation)).last().unwrap()["result"]["probe"]["availability"], "unknown");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn quiet_native_cli_observer_stops_and_reaps_without_cancelling_job() {
    use std::time::{Duration, Instant};
    let f = Fixture::new(); f.submit();
    f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":f.ownership()}));
    fs::write(f.root.path().join("mode"), "quiet").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend")).args(["--config", f.config.to_str().unwrap()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.root.path().join("watching").exists() {
        assert!(Instant::now() < deadline, "quiet watcher never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let stopped = Instant::now();
    writeln!(stdin, "{{\"op\":\"stop_watch\"}}").unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(stopped.elapsed() < Duration::from_secs(2));
    let frames: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    assert_eq!(frames.last().unwrap()["result"]["observation"]["kind"], "unknown", "{frames:?}");
    assert!(!f.calls().lines().any(|line| line.starts_with("cancel ")));
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn generation_slash_is_not_a_path_and_binding_guard_rejects_handoff() {
    assert!(api::generation("fv1-model/61f47a7f95af46d794ca89c3a71f0654"));
    assert!(!api::generation("../../host"));
    let f = Fixture::new(); f.submit();
    let mut options = f.options.clone(); options.instance_generation = "replacement-generation".into();
    fs::write(&f.config, serde_json::to_vec(&options).unwrap()).unwrap();
    assert_eq!(f.attach().last().unwrap()["code"], "acceptance_unknown");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn separate_process_attach_cancel_and_late_terminal_preserve_journal_state() {
    use std::time::{Duration, Instant};
    let f = Fixture::new(); f.submit();
    f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":f.ownership()}));
    fs::write(f.root.path().join("mode"), "quiet").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend")).args(["--config", f.config.to_str().unwrap()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.root.path().join("watching").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let cancel = f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(cancel.last().unwrap()["result"]["acknowledgement"]["acknowledged"], true);
    writeln!(stdin, "{{\"op\":\"stop_watch\"}}").unwrap();
    drop(stdin);
    assert!(child.wait_with_output().unwrap().status.success());
    assert_eq!(f.calls().lines().filter(|line| line.starts_with("cancel ")).count(), 1);
    fs::write(f.root.path().join("mode"), "normal").unwrap();
    f.report(ReportOutcome::Passed, true); f.events(&completed(0));
    let first = f.attach();
    let receipt = first.last().unwrap()["result"]["observation"]["terminal"]["receipt"].clone();
    assert_ne!(receipt, Value::Null, "{first:?}");
    let path = f.options.journal_root.join(&f.intent.correlation).join("state.json");
    let state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(state["cancel_requested"], true);
    assert_eq!(state["binding"]["id"]["job"], "fABC");
    assert_eq!(state["events"].as_array().unwrap().len(), 3);
    let second = f.attach();
    assert_eq!(second.last().unwrap()["result"]["observation"]["terminal"]["receipt"], receipt);
}

#[test]
fn optional_reviewed_sha_remains_distinct_in_wire_and_request_digest() {
    let absent = Suite::DaytimeChecks { plan_digest: "a".repeat(64), base: "b".repeat(40), head: "d".repeat(40), reviewed: None };
    let present = Suite::DaytimeChecks { plan_digest: "a".repeat(64), base: "b".repeat(40), head: "d".repeat(40), reviewed: Some("c".repeat(40)) };
    let absent_json = serde_json::to_value(&absent).unwrap();
    let present_json = serde_json::to_value(&present).unwrap();
    assert_eq!(absent_json["reviewed"], Value::Null);
    assert_eq!(present_json["reviewed"], "c".repeat(40));
    assert_ne!(digest(&serde_json::to_vec(&absent).unwrap()), digest(&serde_json::to_vec(&present).unwrap()));
    let restored: Suite = serde_json::from_value(absent_json).unwrap();
    assert_eq!(restored, absent);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn all_attach_native_phases_stop_and_reap_without_native_cancel() {
    use std::time::{Duration, Instant};
    for mode in ["quiet-jobs", "quiet-info", "quiet-collect"] {
        let f = Fixture::new();
        if mode == "quiet-jobs" { fs::write(f.root.path().join("mode"), "unknown").unwrap(); }
        f.submit();
        f.events(&completed(0));
        fs::write(f.root.path().join("mode"), mode).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_tl3-run-backend")).args(["--config", f.config.to_str().unwrap()])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", json!({"op":"attach","owner":f.intent.owner,"correlation":f.intent.correlation})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f.root.path().join("watching").exists() {
            assert!(Instant::now() < deadline, "phase {mode} was not reached");
            std::thread::sleep(Duration::from_millis(10));
        }
        let start = Instant::now();
        writeln!(stdin, "{{\"op\":\"stop_watch\"}}").unwrap();
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2), "phase {mode} did not stop promptly");
        assert!(!output.stdout.is_empty());
        assert!(!f.calls().lines().any(|call| call.starts_with("cancel ")));
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn collector_exact_frame_and_typed_terminal_payload_verify_all_links() {
    let mut f = Fixture::new();
    f.request.suite = Suite::DaytimeChecks { plan_digest: "a".repeat(64), base: "b".repeat(40), head: "c".repeat(40), reviewed: None };
    let prepared = f.call(json!({"op":"prepare_intent","owner":f.intent.owner,"correlation":f.intent.correlation,"request":f.request}));
    f.intent = serde_json::from_value(prepared.last().unwrap()["result"]["intent"].clone()).unwrap();
    f.submit(); f.report(ReportOutcome::ChecksCompleted, true);
    let report = json!({"schema":"tl3.checks.v2","effect_key":"fixture","base":"b".repeat(40),"head":"c".repeat(40),"reviewed":null,"results":[]});
    let out = &f.intent.output_dir;
    let bytes = format!("trusted host prefix\nTL3_CHECKS {report}\n").into_bytes();
    let stdout = api::durable::blob(&out.join("raw-checks-stdout"), &bytes).unwrap();
    let cleanup = api::durable::blob(&out.join("cleanup.txt"), &fs::read(out.join("cleanup.txt")).unwrap()).unwrap();
    let frames = f.call(json!({"op":"collect_runner","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":{
        "run":f.ownership().run,"stdout":stdout,"runtime_binding":null,"cleanup":cleanup}}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let retained: BlobRef = serde_json::from_value(frames.last().unwrap()["result"]["payload"].clone()).unwrap();
    assert_eq!(fs::read(&retained.path).unwrap(), report.to_string().as_bytes());
    f.events(&completed(0));
    let terminal = f.attach().last().unwrap()["result"]["observation"]["terminal"].clone();
    assert_eq!(terminal["ending"], "passed", "{terminal}");
    let payload = f.call(json!({"op":"daytime_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(payload.last().unwrap()["result"]["report"], report, "{payload:?}");
    fs::write(retained.path, b"{}").unwrap();
    let corrupt = f.call(json!({"op":"daytime_payload","owner":f.intent.owner,"correlation":f.intent.correlation,"terminal":terminal}));
    assert_eq!(corrupt.last().unwrap()["kind"], "error");
    for raw in ["no frame\n".to_owned(), "TL3_CHECKS {}\n".to_owned(), format!("TL3_CHECKS {report}\nTL3_CHECKS {report}\n")] {
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
        fs::write(f.root.path().join("mode"), "unknown").unwrap(); f.submit();
        let path = f.root.path().join("jobspec.json");
        let mut spec: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match field {
            "resources" => spec["resources"][0]["count"] = json!(100),
            "placement" => spec["attributes"]["system"]["constraints"] = json!("host:sparky"),
            _ => spec["attributes"]["system"]["duration"] = json!("999s"),
        }
        fs::write(path, spec.to_string()).unwrap();
        assert_eq!(f.attach().last().unwrap()["result"]["observation"]["kind"], "unknown");
    }
}

impl Fixture {
    fn podman(&mut self, mode: ContainerMode, prerequisites: bool) {
        let p = self.root.path();
        let image = ImageIdentity { inventory: api::durable::blob(&p.join("image-map.json"), &serde_json::to_vec(&json!({"profiles":[{
            "profile":"fixture","registry_ref":format!("fixture.invalid/test@sha256:{}", "a".repeat(64)),"manifest_digest":format!("sha256:{}", "a".repeat(64)),
            "config_digest":format!("sha256:{}", "b".repeat(64)),"platform":"linux/amd64"}]})).unwrap()).unwrap(),
            profile: "fixture".into(), reference: format!("fixture.invalid/test@sha256:{}", "a".repeat(64)), index_digest: None,
            manifest_digest: format!("sha256:{}", "a".repeat(64)), config_digest: format!("sha256:{}", "b".repeat(64)), platform: "linux/amd64".into() };
        let trusted = p.join("tl3_harness"); fs::create_dir(&trusted).unwrap(); immutable(&trusted.join("runner.py"), b"# trusted fixture\n", false);
        let cleanup = p.join("cleanup-helper"); immutable(&cleanup, b"#!/bin/sh\ncat\n", true);
        self.request.runtime.runtime = ExecutionRuntime::RootlessPodman { mode, executable: "/usr/bin/podman".into(), image: image.reference.clone(), trusted_root: trusted,
            interpreter: "/usr/bin/python3".into(), memory_limit_bytes: (mode == ContainerMode::OwnScope).then(|| 1_073_741_824.try_into().unwrap()) };
        self.request.runtime.expected_image = Some(image.reference.clone());
        self.request.runtime.image = Some(image.clone());
        self.request.runtime.cleanup = Some(CleanupProgram { executable: cleanup, executable_digest: digest(b"#!/bin/sh\ncat\n"), arguments: vec![] });
        let bytes = serde_json::to_vec(&RuntimePreflight { schema: "tl3.runtime_preflight.v1".into(), architecture: "x86_64".into(), image_identity: Some(image.reference.clone()),
            image_present: true, identity_verified: true, image: Some(image), mode: Some(mode), no_cgroups_supported: prerequisites, init_available: prerequisites, user_scope_supported: prerequisites }).unwrap();
        immutable(&p.join("podman-preflight.json"), &bytes, false);
        self.request.runtime.preflight = BlobRef { path: p.join("podman-preflight.json"), sha256: digest(&bytes), bytes: bytes.len() as u64 };
        self.request.environment.insert("HOME".into(), "/home/rob".into());
        self.request.environment.insert("XDG_RUNTIME_DIR".into(), "/run/user/1000".into());
        self.intent.request_digest = digest(&serde_json::to_vec(&self.request).unwrap());
    }

    fn container_ownership(&self) -> RuntimeOwnership {
        let mut owned = self.ownership();
        let mode = match self.request.runtime.runtime { ExecutionRuntime::RootlessPodman { mode, .. } => mode, _ => unreachable!() };
        let unit = format!("tl3-{}-fABC-owned.scope", self.intent.correlation);
        let scope = ScopeIdentity { cgroup: format!("/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}"), unit, invocation_id: "c".repeat(32) };
        let labels = [("tl3.backend", "flux"), ("tl3.uri", self.intent.uri.as_str()), ("tl3.instance", self.intent.instance.as_str()),
            ("tl3.job", "fABC"), ("tl3.owner", self.intent.owner.as_str()), ("tl3.correlation", self.intent.correlation.as_str()), ("tl3.request_digest", self.intent.request_digest.as_str())]
            .into_iter().map(|(key, value)| (key.to_owned(), value.to_owned())).collect();
        owned.container = Some(ContainerOwnership { mode, id: "d".repeat(64), labels, scope: (mode == ContainerMode::OwnScope).then_some(scope) });
        owned
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn selected_mode_refuses_missing_prerequisites_without_fallback() {
    for mode in [ContainerMode::InJob, ContainerMode::OwnScope] {
        let mut f = Fixture::new(); f.podman(mode, false);
        assert_eq!(f.submit()["terminal"]["ending"], "refused");
        assert!(!f.calls().contains("job submit "));
    }
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn own_scope_cleanup_requires_exact_generation_absence_and_unpopulated_scope() {
    let mut f = Fixture::new(); f.podman(ContainerMode::OwnScope, true); f.submit();
    let owned = f.container_ownership();
    let frames = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    assert_eq!(frames.last().unwrap()["kind"], "result", "{frames:?}");
    let container = owned.container.as_ref().unwrap();
    let scope = container.scope.as_ref().unwrap();
    let mut facts = CleanupEvidence { schema: "tl3.cleanup_evidence.v1".into(), ownership: owned.clone(), launcher: ProcessStop::Absent,
        process_group_probe_errno: Some(3), container: Some(ContainerStop { id: container.id.clone(), remove_exit: 0, exists_exit: 1, sweep_ids: vec![container.id.clone()],
            scope: Some(ScopeStop { identity: scope.clone(), observed_invocation_id: Some(scope.invocation_id.clone()), cgroup_events: Some("populated 1\nfrozen 0\n".into()), cgroup_procs: String::new(), active_state: "inactive".into() }) }) };
    for case in ["populated", "wrong-generation", "podman-error", "stopped"] {
        let stop = facts.container.as_mut().unwrap();
        stop.exists_exit = if case == "podman-error" { 125 } else { 1 };
        let scope_stop = stop.scope.as_mut().unwrap();
        scope_stop.cgroup_events = Some(if case == "populated" { "populated 1\n" } else { "populated 0\n" }.into());
        scope_stop.observed_invocation_id = Some(if case == "wrong-generation" { "e".repeat(32) } else { scope.invocation_id.clone() });
        let reference = api::durable::blob(&f.intent.output_dir.join("typed-cleanup.json"), &serde_json::to_vec(&facts).unwrap()).unwrap();
        let frames = f.call(json!({"op":"retain_cleanup","owner":f.intent.owner,"correlation":f.intent.correlation,"evidence":reference}));
        assert_eq!(frames.last().unwrap()["result"]["cleanup"]["stopped"], case == "stopped", "{case}: {frames:?}");
    }
    let mut changed = owned; changed.container.as_mut().unwrap().id = "f".repeat(64);
    let conflict = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":changed}));
    assert_eq!(conflict.last().unwrap()["kind"], "error");
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn own_scope_runtime_mapping_compares_golden_dl380_sets_and_effective_cap() {
    let mut f = Fixture::new(); f.podman(ContainerMode::OwnScope, true);
    f.request.reservation.cpus = 4.try_into().unwrap(); f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap()); f.submit();
    let owned = f.container_ownership();
    f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    let allocation = api::durable::blob(&f.intent.output_dir.join("R.json"), &serde_json::to_vec(&json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":"21-23,30"}}],"nodelist":["dl380g10"]}})).unwrap()).unwrap();
    let topology = api::durable::blob(&f.intent.output_dir.join("topology.json"), &serde_json::to_vec(&TopologyMapping { run: owned.run.clone(), request_digest: f.intent.request_digest.clone(), ranks: vec![RankMapping { rank: 0, host: "dl380g10".into(), cores: [21,22,23,30].into_iter().map(|i| CoreMapping { flux_core: i, os_pus: format!("{i},{}", i+40) }).collect() }] }).unwrap()).unwrap();
    let scope = owned.container.as_ref().unwrap().scope.as_ref().unwrap();
    let mut facts = RuntimeBindingInput { run: owned.run.clone(), request_digest: f.intent.request_digest.clone(), allocation, topology,
        host: ProcessPlacement { rank: 0, host: "dl380g10".into(), allowed_cpus: "21-23,30,61-63,70".into(), cgroup: "0::/user.slice/native-broker".into() },
        container: Some(ProcessPlacement { rank: 0, host: "dl380g10".into(), allowed_cpus: "21-23,30,61-63,70".into(), cgroup: format!("0::{}", scope.cgroup) }),
        container_scope: Some(ContainerScopeFacts { unit: scope.unit.clone(), invocation_id: scope.invocation_id.clone(), cgroup: format!("0::{}", scope.cgroup), cpuset_effective: "21-23,30,61-63,70".into(), memory_max: Some(1_073_741_824), memory_swap_max: Some(0), container_id: owned.container.as_ref().unwrap().id.clone() }) };
    let frames = f.call(json!({"op":"validate_runtime_binding","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":facts}));
    assert_eq!(frames.last().unwrap()["result"]["qualification"]["qualified"], true, "{frames:?}");
    facts.container_scope.as_mut().unwrap().memory_max = None;
    let frames = f.call(json!({"op":"validate_runtime_binding","owner":f.intent.owner,"correlation":f.intent.correlation,"facts":facts}));
    assert_eq!(frames.last().unwrap()["result"]["qualification"]["qualified"], false);
}

#[test]
#[ignore = "requires /mnt/shared; execute through an admitted PrismaBuild action with --ignored"]
fn cancellation_uses_independent_cleanup_without_runner_report_after_wrapper_loss() {
    let mut f = Fixture::new(); f.podman(ContainerMode::OwnScope, true);
    let executable = f.root.path().join("independent-cleanup");
    let program = b"#!/bin/sh\ncat \"$1\"\n";
    immutable(&executable, program, true);
    f.request.runtime.cleanup = Some(CleanupProgram { executable, executable_digest: digest(program), arguments: vec![f.intent.output_dir.join("trusted-stop-facts.json").to_string_lossy().into_owned()] });
    f.intent.request_digest = digest(&serde_json::to_vec(&f.request).unwrap()); f.submit();
    let owned = f.container_ownership();
    let retained = f.call(json!({"op":"retain_runtime","owner":f.intent.owner,"correlation":f.intent.correlation,"ownership":owned}));
    assert_eq!(retained.last().unwrap()["kind"], "result");
    let container = owned.container.as_ref().unwrap(); let scope = container.scope.as_ref().unwrap();
    let facts = CleanupEvidence { schema: "tl3.cleanup_evidence.v1".into(), ownership: owned.clone(), launcher: ProcessStop::Absent,
        process_group_probe_errno: Some(3), container: Some(ContainerStop { id: container.id.clone(), remove_exit: 0, exists_exit: 1, sweep_ids: vec![container.id.clone()],
            scope: Some(ScopeStop { identity: scope.clone(), observed_invocation_id: None, cgroup_events: None, cgroup_procs: String::new(), active_state: "not-found".into() }) }) };
    fs::write(f.intent.output_dir.join("trusted-stop-facts.json"), serde_json::to_vec(&facts).unwrap()).unwrap();
    let cancel = f.call(json!({"op":"cancel","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(cancel.last().unwrap()["result"]["acknowledgement"]["acknowledged"], true, "{cancel:?}");
    assert!(!f.intent.output_dir.join("report.json").exists());
    f.events(&[json!({"name":"exception","context":{"type":"cancel","severity":0}}),json!({"name":"clean"})]);
    let frames = f.attach();
    assert_eq!(frames.last().unwrap()["result"]["observation"]["terminal"]["ending"], "cancelled", "{frames:?}");
    // The cleanup entry uses retained generation/labels even after the broker pin changes.
    let mut options = f.options.clone(); options.instance_generation = "replacement-generation".into();
    fs::write(&f.config, serde_json::to_vec(&options).unwrap()).unwrap();
    let cleanup = f.call(json!({"op":"cleanup_runtime","owner":f.intent.owner,"correlation":f.intent.correlation}));
    assert_eq!(cleanup.last().unwrap()["result"]["cleanup"]["stopped"], true, "{cleanup:?}");
}
