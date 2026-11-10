use super::*;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tl3_controller::{
    adapters::run_backend::flux::Flux,
    run_backend::{Observation, Owner, RunBackend, Submission, WatchStop},
};

pub fn allocation(value: &Value, cores: usize, host: &str) -> Result<()> {
    let rows = value["execution"]["R_lite"]
        .as_array()
        .ok_or("allocation R_lite missing")?;
    if value["version"] != 1
        || rows.len() != 1
        || rows[0]["rank"] != "0"
        || !value["execution"]["nodelist"]
            .as_array()
            .is_some_and(|hosts| hosts.len() == 1 && hosts[0] == host)
        || rows[0]["children"].get("gpu").is_some_and(|gpu| gpu != "")
    {
        return Err("SHAPE allocation host/rank/GPU mismatch".into());
    }
    let text = rows[0]["children"]["core"]
        .as_str()
        .ok_or("allocated core IDs missing")?;
    let mut ids = std::collections::BTreeSet::new();
    for span in text.split(',') {
        let (first, last) = span.split_once('-').unwrap_or((span, span));
        let first: u32 = first.parse()?;
        let last: u32 = last.parse()?;
        if first > last || last > 65535 {
            return Err("invalid allocation core range".into());
        }
        for id in first..=last {
            if !ids.insert(id) {
                return Err("duplicate allocation core".into());
            }
        }
    }
    if ids.len() != cores {
        return Err("allocated core count mismatch".into());
    }
    Ok(())
}

fn flushed_events(raw: &[u8]) -> Result<Vec<Value>> {
    // Only complete flushed JSON lines count. A writer's partial final line is
    // retried on the next observation, never promoted to a readiness marker.
    let mut result = vec![];
    for line in raw.split_inclusive(|c| *c == b'\n') {
        if line.ends_with(b"\n") {
            result.push(serde_json::from_slice(line)?);
        }
    }
    Ok(result)
}

fn journal_events(path: &Path) -> Result<Vec<Value>> {
    flushed_events(&durable::read(path)?)
}

fn ready_events(events: &[Value], id: &str, name: &str) -> bool {
    let mut cursor = 0;
    for event in events {
        let matches = match cursor {
            0 => {
                event["event"] == "container_processes"
                    && event["Id"] == id
                    && event["Pid"].as_u64().is_some_and(|pid| pid > 1)
                    && event["ConmonPid"].as_u64().is_some_and(|pid| pid > 1)
            }
            1 => {
                event["event"] == "launch_binding"
                    && event["payload_allowed"] == true
                    && event["ownership"]["container"]["id"] == id
            }
            2 => event["event"] == "qualification" && event["qualified"] == true,
            3 => {
                event["event"] == "payload_released"
                    && event["marker"] == "tl3.payload_released.v1"
                    && event["container"] == name
            }
            _ => false,
        };
        if matches {
            cursor += 1;
        }
    }
    cursor == 4
}

fn ready(record: &durable::Record) -> Result<bool> {
    for owned in &record.runtime_ownerships {
        let Some(container) = &owned.container else {
            continue;
        };
        if !record.container_launches.contains_key(&owned.execution) {
            continue;
        }
        let path = record
            .intent
            .output_dir
            .join(format!("{}.runtime.jsonl", owned.execution));
        if !path.exists() {
            continue;
        }
        let events = journal_events(&path)?;
        if ready_events(&events, &container.id, &container.name) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn collect(
    inputs: &Inputs,
    request: api::Request,
    options: api::Options,
    kind: &str,
) -> Result<evidence::CanaryEvidence> {
    match collect_inner(inputs, request, options.clone(), kind) {
        Ok(result) => Ok(result),
        Err(error) => {
            let root = inputs.out.join(format!("canary-{kind}"));
            fs::create_dir_all(&root)?;
            let failure = durable::blob(
                &root.join("failure.json"),
                &serde_json::to_vec(&json!({"reason":error.to_string()}))?,
            )?;
            let owner = Owner {
                owner: "x3-qualification".into(),
                correlation: format!(
                    "x3-{}-{kind}",
                    &durable::digest(inputs.out.as_os_str().as_encoded_bytes())[..24]
                ),
            };
            let record = durable::Journal::new(options.journal_root)?
                .load(&owner)
                .ok();
            let terminal = record.as_ref().and_then(|r| r.terminal.clone());
            let result=evidence::CanaryEvidence {kind:kind.into(),status:"UNKNOWN".into(),
                job:record.as_ref().and_then(|r|r.binding.as_ref()).map(|binding|json!({"id":binding.id.job,"owner":owner,"instance_receipts":record.as_ref().and_then(|r|r.instance_observation.as_ref())})),
                ending:terminal.as_ref().map(|t|t.ending),terminal_receipt:terminal.map(|t|t.receipt),cleanup:None,
                executions:vec![],artifacts:vec![],checks:vec![evidence::Check {name:"collection_complete".into(),pass:false,evidence:vec![failure]}]};
            durable::atomic(
                &root.join("result.json"),
                &serde_json::to_vec_pretty(&result)?,
            )?;
            Ok(result)
        }
    }
}

fn collect_inner(
    inputs: &Inputs,
    request: api::Request,
    options: api::Options,
    kind: &str,
) -> Result<evidence::CanaryEvidence> {
    let root = inputs.out.join(format!("canary-{kind}"));
    if !root.exists() {
        fs::create_dir(&root)?;
    }
    let retained = root.join("result.json");
    if retained.exists() {
        let result: evidence::CanaryEvidence = serde_json::from_slice(&durable::read(&retained)?)?;
        evidence::verify_references(&serde_json::to_value(&result)?)?;
        if result.status != "UNKNOWN" {
            return Ok(result);
        }
    }
    let owner = Owner {
        owner: "x3-qualification".into(),
        correlation: format!(
            "x3-{}-{kind}",
            &durable::digest(inputs.out.as_os_str().as_encoded_bytes())[..24]
        ),
    };
    let flux = Flux::new(options.clone())?;
    let endpoint = options.resolved_uri()?;
    let intent = flux.prepare_intent(&request, &owner)?;
    if !root.join("intent.json").exists() {
        write(&root.join("intent.json"), &intent)?;
    }
    let absent_binding = if kind == "launch_failure" {
        let (manifest, reference) = api::prepared::read_bundle(&request.runner.manifest)?;
        if reference.sha256 != request.runner.manifest_digest {
            return Err("launch failure manifest digest differs".into());
        }
        let prepared_root = inputs.out.join("prepared");
        let prepared: Value =
            serde_json::from_slice(&durable::read(&prepared_root.join("result.json"))?)?;
        let preflight = &prepared["absent_interpreter_preflight"];
        let proof: api::BlobRef = serde_json::from_value(preflight["proof"].clone())?;
        let observed: Value =
            serde_json::from_slice(&durable::checked_blob(&proof, &prepared_root)?)?;
        let binding = matches!(&request.runtime.runtime,
            api::ExecutionRuntime::RootlessPodman { image, interpreter, .. }
                if image == &inputs.test_image && interpreter == &manifest.container_interpreter)
            && preflight["image"] == inputs.test_image
            && absent_interpreter_binding(&manifest.container_interpreter, &observed);
        Some((
            binding,
            manifest.container_interpreter,
            observed,
            vec![reference, proof],
        ))
    } else {
        None
    };
    let submission = flux.submit(request.clone(), intent.clone())?;
    if let Submission::Refused { terminal } = submission {
        return Err(format!(
            "canary refused before native dispatch: {:?}",
            terminal.ending
        )
        .into());
    }
    let journal = durable::Journal::new(options.journal_root.clone())?;
    let mut cancel_problem = None;
    let observation = std::thread::scope(|scope| -> Result<api::Observation> {
        let (send, receive) = mpsc::sync_channel(1);
        let reader = flux.clone();
        let observed_owner = owner.clone();
        scope.spawn(move || {
            let result = reader.attach(&observed_owner, &WatchStop::default(), &mut |_| Ok(()));
            let _ = send.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(inputs.readiness_seconds);
        let mut attempted = root.join("cancel-intent.json").exists();
        loop {
            match receive.recv_timeout(Duration::from_secs(1)) {
                Ok(result) => return Ok(result?),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("backend observer disconnected".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if kind == "cancel" && !attempted {
                let record = journal.load(&owner)?;
                if ready(&record)? {
                    write(
                        &root.join("cancel-intent.json"),
                        &json!({"owner":owner,"binding":record.binding,"readiness":"flushed qualified payload release marker"}),
                    )?;
                    attempted = true;
                    let ack = flux.cancel(&owner)?;
                    write(&root.join("cancel-ack.json"), &ack)?;
                    if !ack.acknowledged {
                        cancel_problem = Some("owned cancel was not acknowledged");
                    }
                } else if Instant::now() >= deadline {
                    attempted = true;
                    cancel_problem = Some(
                        "readiness deadline exceeded; collect natural ending without cancellation",
                    );
                }
            }
        }
    })?;
    let (binding, terminal) = match observation {
        Observation::Terminal { binding, terminal } => {
            (binding.ok_or("terminal missing job binding")?, terminal)
        }
        Observation::Unknown { .. } => {
            return Err("backend observation unknown; owned job retained".into());
        }
    };
    let receipt: api::Receipt = serde_json::from_slice(&durable::checked_blob(
        &terminal.receipt,
        &intent.output_dir,
    )?)?;
    let cleanup = flux.cleanup_runtime(&owner)?;
    let record = journal.load(&owner)?;
    let expected = match kind {
        "launch_failure" => api::Ending::InfraLost,
        "cancel" => api::Ending::Cancelled,
        _ => api::Ending::Passed,
    };
    let mut checks = vec![];
    let mut add = |name: &str, pass: bool, evidence: Vec<api::BlobRef>| {
        checks.push(evidence::Check {
            name: name.into(),
            pass,
            evidence,
        })
    };
    add(
        "typed_ending",
        terminal.ending == expected,
        vec![terminal.receipt.clone()],
    );
    add(
        "instance_receipts_complete",
        record.instance_observation.as_ref().is_some_and(|receipt| {
            receipt
                .broker_starttime
                .as_ref()
                .is_some_and(|s| !s.is_empty())
                && receipt
                    .loaded_config
                    .as_ref()
                    .zip(receipt.config_sha256.as_ref())
                    .is_some_and(|(config, hash)| config.sha256 == *hash)
        }),
        vec![],
    );
    add(
        "clean_event",
        receipt
            .native_events
            .last()
            .is_some_and(|e| matches!(e.fact, api::NativeFact::Clean)),
        vec![terminal.receipt.clone()],
    );
    add(
        "verified_cleanup",
        cleanup.stopped && cleanup.pending_executions.is_empty() && !cleanup.evidence.is_empty(),
        cleanup.evidence.clone(),
    );
    let scratch: Vec<_> = receipt
        .evidence
        .iter()
        .filter(|r| {
            r.path
                .file_name()
                .is_some_and(|p| p.to_string_lossy().starts_with("scratch-absence-"))
        })
        .cloned()
        .collect();
    let scratch_status: Vec<_> = scratch
        .iter()
        .filter(|r| r.path.to_string_lossy().ends_with(".status.json"))
        .collect();
    let mut scratch_pass = !scratch_status.is_empty();
    for status in scratch_status {
        let exit: Value =
            serde_json::from_slice(&durable::checked_blob(status, &intent.output_dir)?)?;
        let stdout = status.path.with_file_name(
            status
                .path
                .file_name()
                .ok_or("scratch status name")?
                .to_string_lossy()
                .replace(".status.json", ".stdout"),
        );
        let observed = scratch
            .iter()
            .find(|r| r.path == stdout)
            .ok_or("scratch observation missing")?;
        let observed: Value =
            serde_json::from_slice(&durable::checked_blob(observed, &intent.output_dir)?)?;
        scratch_pass &= scratch_observed(&exit, &observed);
    }
    add("scratch_absence", scratch_pass, scratch);
    let grant = record
        .allocation_grant
        .as_ref()
        .ok_or("allocation grant missing")?;
    let allocation_value: Value = serde_json::from_slice(&durable::checked_blob(
        &grant.allocation,
        &intent.output_dir,
    )?)?;
    add(
        "shape_allocation",
        allocation(&allocation_value, 1, "dl380g10").is_ok(),
        vec![grant.allocation.clone()],
    );
    if kind == "cancel" {
        let cancel=receipt.native_events.iter().position(|e|matches!(&e.fact,api::NativeFact::Exception { exception_type,.. } if exception_type=="cancel"));
        let finish = receipt
            .native_events
            .iter()
            .position(|e| matches!(e.fact, api::NativeFact::Finish { .. }));
        add(
            "owned_cancel_after_readiness",
            cancel_problem.is_none()
                && root.join("cancel-ack.json").exists()
                && cancel.zip(finish).is_some_and(|(c, f)| c < f),
            vec![],
        );
    }
    if kind == "launch_failure" {
        let report: api::RunnerReport = serde_json::from_slice(&durable::checked_blob(
            receipt
                .report
                .as_ref()
                .ok_or("launch failure report missing")?,
            &intent.output_dir,
        )?)?;
        let mut cause = report.outcome == api::ReportOutcome::PostBeginLaunchFailure
            && report.worker_payload.is_none()
            && report.payload.is_none()
            && !report.launch_failures.is_empty();
        let (binding, interpreter, absent, mut proofs) =
            absent_binding.ok_or("missing absent interpreter binding")?;
        cause &= binding;
        for failure in &report.launch_failures {
            let events = flushed_events(&durable::checked_blob(
                &failure.process_record,
                &intent.output_dir,
            )?)?;
            cause &= absent_interpreter_failure(&events)
                && absent_interpreter_binding(&interpreter, &absent);
            proofs.push(failure.process_record.clone());
        }
        add("post_begin_launch_failure", cause, proofs);
    }
    if kind == "success" {
        let report = flux.daytime_payload(&owner, &terminal)?;
        let values = serde_json::to_value(report)?;
        let results = values["results"].as_array().ok_or("checks result absent")?;
        add(
            "real_pytest_base_red_head_green",
            success_results(results, record.container_launches.len())
                && record.container_launches.keys().all(|execution| {
                    intent
                        .output_dir
                        .join(format!("{execution}.runtime.jsonl"))
                        .is_file()
                }),
            vec![],
        );
    }
    let absence = job_absence(inputs, &options, &binding.id.job, &root)?;
    add("job_cgroup_absent", absence.0, vec![absence.1]);
    let mut executions = vec![];
    for execution in record.container_launches.keys() {
        let path = intent.output_dir.join(format!("{execution}.runtime.jsonl"));
        if path.exists() {
            executions.push(json!({"id":execution,"journal":identity(&path)?}));
        }
    }
    let status = if checks.iter().all(|c| c.pass) {
        "PASS"
    } else {
        "FAIL"
    };
    let result = evidence::CanaryEvidence {
        kind: kind.into(),
        job: Some(json!({"id":binding.id.job,"owner":owner,
        "request_sha256":intent.request_digest,"request":identity(&intent.output_dir.join("request.json"))?,
        "jobspec":record.jobspec,"instance_receipts":[{"endpoint":endpoint,"observation":record.instance_observation}],"allocation_R":grant.allocation,
        "exit":receipt.native_events.iter().find_map(|e|if let api::NativeFact::Finish {status}=&e.fact {Some(status)}else{None}),
        "events":receipt.native_events,"logs":[identity(&intent.output_dir.join("stdout"))?,identity(&intent.output_dir.join("stderr"))?]})),
        status: status.into(),
        ending: Some(terminal.ending),
        terminal_receipt: Some(terminal.receipt),
        cleanup: Some(cleanup),
        executions,
        artifacts: vec![],
        checks,
    };
    durable::atomic(&retained, &serde_json::to_vec_pretty(&result)?)?;
    Ok(result)
}

fn absent_interpreter_binding(interpreter: &Path, observed: &Value) -> bool {
    interpreter == Path::new("/opt/tl3/bin/python-canary2-absent")
        && observed["interpreter"] == "/opt/tl3/bin/python-canary2-absent"
        && observed["absent"] == true
}

fn absent_interpreter_failure(events: &[Value]) -> bool {
    !events.iter().any(|e| e["event"] == "payload_released")
        && events
            .iter()
            .filter(|e| e["event"] == "launcher_exit")
            .count()
            == 1
        && events.iter().any(|e| {
            e["event"] == "launcher_exit"
                && e["exit_code"] == 1
                && e["stderr_tail"]
                    == "ERROR (catatonit:2): failed to exec pid1: No such file or directory\n"
        })
}

fn scratch_observed(status: &Value, observed: &Value) -> bool {
    status["code"] == 0
        && status.get("signal") == Some(&Value::Null)
        && observed["schema"] == "tl3.scratch_cleanup.v1"
        && observed["observation"] == json!({"kind":"absent","errno":2})
}

fn success_results(results: &[Value], executions: usize) -> bool {
    executions == 2
        && !results.is_empty()
        && results.iter().all(|r| {
            r["verdict"] == "pass"
                && r["proof"]["declaration"]["red_message"] == "sandbox value must be 2"
                && r["tests"].as_array().is_some_and(|tests| {
                    !tests.is_empty()
                        && tests.iter().all(|t| {
                            t["base"] == "assertion"
                                && t["head"] == "passed"
                                && t["passed"] == true
                                && t["test"].as_str().is_some_and(|test| {
                                    matches!(
                                        r["proof"]["red_message_matches"][test].as_str(),
                                        Some("message" | "source-assertion")
                                    )
                                })
                        })
                })
        })
}

pub fn job_absence(
    inputs: &Inputs,
    options: &api::Options,
    id: &str,
    root: &Path,
) -> Result<(bool, api::BlobRef)> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == 'ƒ') {
        return Err("invalid owned job id".into());
    }
    let uid = unsafe { libc::geteuid() };
    let control =
        format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/flux.slice");
    let path = format!("{control}/flux-jobs.slice/shell-{id}:sys.service");
    let (exit, _) = admission::capture(
        options,
        &[
            "exec".into(),
            "-r0".into(),
            "/bin/sh".into(),
            "-c".into(),
            "test -d \"$1\" && test ! -e \"$2\"".into(),
            "x3-read-only".into(),
            control.clone(),
            path.clone(),
        ],
        &root.join("absence.log"),
    )?;
    let reference = durable::blob(
        &root.join("cgroup-absence.json"),
        &serde_json::to_vec(
            &json!({"head":inputs.head,"job_id":id,"path":path,"positive_control":control,"exit":exit,"absent":exit==0}),
        )?,
    )?;
    Ok((exit == 0, reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_readiness_requires_complete_flushed_ordered_bound_markers() {
        let events = vec![
            json!({"event":"container_processes","Id":"id","Pid":42,"ConmonPid":43}),
            json!({"event":"launch_binding","payload_allowed":true,"ownership":{"container":{"id":"id"}}}),
            json!({"event":"qualification","qualified":true}),
            json!({"event":"payload_released","marker":"tl3.payload_released.v1","container":"name"}),
        ];
        let mut raw = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        assert!(!ready_events(&flushed_events(&raw).unwrap(), "id", "name"));
        raw.push(b'\n');
        assert!(ready_events(&flushed_events(&raw).unwrap(), "id", "name"));
        let mut wrong = events.clone();
        wrong.swap(1, 2);
        assert!(!ready_events(&wrong, "id", "name"));
        assert!(!ready_events(&events, "foreign", "name"));
    }
    #[test]
    fn lifecycle_predicates_require_the_specific_causes_and_both_executions() {
        let row = json!({"verdict":"pass","proof":{"declaration":{"red_message":"sandbox value must be 2"},"red_message_matches":{"test_value.py::test_value":"message"}},"tests":[{"test":"test_value.py::test_value","base":"assertion","head":"passed","passed":true,"message":""}]});
        assert!(success_results(std::slice::from_ref(&row), 2));
        assert!(!success_results(std::slice::from_ref(&row), 1));
        let mut wrong = row.clone();
        wrong["proof"]["declaration"]["red_message"] = "unrelated assertion".into();
        assert!(!success_results(&[wrong], 2));
        let mut wrong = row.clone();
        wrong["proof"]["red_message_matches"]["test_value.py::test_value"] = "mismatch".into();
        assert!(!success_results(&[wrong], 2));
        let mut wrong = row;
        wrong["tests"][0]["head"] = "assertion".into();
        assert!(!success_results(&[wrong], 2));
        let absent = json!({"interpreter":"/opt/tl3/bin/python-canary2-absent","absent":true});
        assert!(absent_interpreter_binding(
            Path::new("/opt/tl3/bin/python-canary2-absent"),
            &absent
        ));
        for path in ["/opt/tl3/bin/python", "/different/absent"] {
            assert!(!absent_interpreter_binding(Path::new(path), &absent));
        }
        assert!(!absent_interpreter_binding(
            Path::new("/opt/tl3/bin/python-canary2-absent"),
            &json!({"interpreter":"/opt/tl3/bin/python-canary2-absent","absent":false})
        ));
        let historical: Value = serde_json::from_str(include_str!(
            "../../qualification/fx-exact42b/canary2-acceptance.json"
        ))
        .unwrap();
        for execution in historical["executions"].as_array().unwrap() {
            let exit = execution["launcher_exit"].clone();
            let mut raw = serde_json::to_vec(&exit).unwrap();
            raw.push(b'\n');
            let events = flushed_events(&raw).unwrap();
            assert!(absent_interpreter_failure(&events));
            assert!(!absent_interpreter_failure(&[
                exit.clone(),
                json!({"event":"payload_released"})
            ]));
            assert!(!absent_interpreter_failure(&[exit.clone(), exit.clone()]));
            for (code, message) in [
                (0, exit["stderr_tail"].as_str().unwrap()),
                (127, exit["stderr_tail"].as_str().unwrap()),
                (
                    1,
                    "ERROR (catatonit:2): failed to exec pid1: Permission denied\n",
                ),
                (
                    1,
                    "prefix ERROR (catatonit:2): failed to exec pid1: No such file or directory\n",
                ),
                (
                    127,
                    "crun: /opt/tl3/bin/python-canary2-absent: No such file or directory",
                ),
            ] {
                let mut wrong = exit.clone();
                wrong["exit_code"] = code.into();
                wrong["stderr_tail"] = message.into();
                assert!(!absent_interpreter_failure(&[wrong]));
            }
        }
        assert!(!absent_interpreter_failure(&[]));
    }

    #[test]
    fn scratch_absence_requires_successful_status_and_absence_contents() {
        let status = json!({"code":0,"signal":null});
        let observed =
            json!({"schema":"tl3.scratch_cleanup.v1","observation":{"kind":"absent","errno":2}});
        assert!(scratch_observed(&status, &observed));
        assert!(!scratch_observed(
            &json!({"code":1,"signal":null}),
            &observed
        ));
        assert!(!scratch_observed(&json!({"code":0,"signal":9}), &observed));
        assert!(!scratch_observed(
            &status,
            &json!({"schema":"tl3.scratch_cleanup.v1","observation":{"kind":"present"}})
        ));
        assert!(!scratch_observed(&status, &json!({})));
    }

    #[test]
    fn allocation_uses_counts_and_allowed_host_without_identity_pins() {
        let fixture = json!({"version":1,"execution":{"R_lite":[{"rank":"0","children":{"core":"7"}}],"nodelist":["dl380g10"]}});
        assert!(allocation(&fixture, 1, "dl380g10").is_ok());
        assert_eq!(
            allocation(&fixture, 2, "dl380g10").unwrap_err().to_string(),
            "allocated core count mismatch"
        );
        let mut other = fixture.clone();
        other["execution"]["R_lite"][0]["children"]["core"] = "19".into();
        assert!(allocation(&other, 1, "dl380g10").is_ok());
        other["execution"]["R_lite"][0]["children"]["gpu"] = "0".into();
        assert_eq!(
            allocation(&other, 1, "dl380g10").unwrap_err().to_string(),
            "SHAPE allocation host/rank/GPU mismatch"
        );
    }
}
