//! Serial non-backend preparation jobs, with durable native ownership.
use super::*;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn options(inputs: &Inputs, binary_root: &Path) -> Result<api::Options> {
    let instance = Path::new("/mnt/shared/flux-migration/INSTANCE");
    let locator = fs::read_to_string(instance)?;
    let fields: Vec<_> = locator.split_whitespace().collect();
    if fields.len() != 2 || !api::absolute(Path::new(fields[0])) {
        return Err("invalid INSTANCE locator".into());
    }
    let prefix = Path::new(fields[0]);
    let mut environment = tl3_controller::production::flux_job_environment();
    environment.insert(
        "PATH".into(),
        format!(
            "{}/install/bin:{}/deps/bin:/usr/bin:/bin",
            prefix.display(),
            prefix.display()
        ),
    );
    environment.insert(
        "LD_LIBRARY_PATH".into(),
        format!(
            "{}/install/lib:{}/deps/lib",
            prefix.display(),
            prefix.display()
        ),
    );
    let journal = inputs.out.join("journal");
    fs::create_dir_all(&journal)?;
    let options = api::Options {
        executable: prefix.join("install/bin/flux"),
        arguments: vec![],
        environment,
        instance_file: instance.into(),
        journal_root: journal.clone(),
        bootstrap: api::HostBootstrap {
            cli: binary_root.join("tl3-run-backend"),
            config: journal.join("options.json"),
        },
    };
    durable::atomic(&options.bootstrap.config, &serde_json::to_vec(&options)?)?;
    Ok(options)
}

pub fn capture(options: &api::Options, args: &[String], log: &Path) -> Result<(i32, Vec<u8>)> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(&options.executable)
        .args(&options.arguments)
        .args(args)
        .env_clear()
        .envs(&options.environment)
        .env("FLUX_URI", options.resolved_uri()?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let stdout = child.stdout.take().ok_or("native stdout")?;
    let stderr = child.stderr.take().ok_or("native stderr")?;
    let reader = |stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut raw = Vec::new();
            stream.take(8 * 1024 * 1024 + 1).read_to_end(&mut raw)?;
            Ok(raw)
        })
    };
    let stdout = reader(Box::new(stdout));
    let stderr = reader(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(55);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            // Observation process only. Never signal the native job.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            child.wait()?;
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = stdout.join().map_err(|_| "stdout reader failed")??;
    let errors = stderr.join().map_err(|_| "stderr reader failed")??;
    if output.len() > 8 * 1024 * 1024 || errors.len() > 8 * 1024 * 1024 {
        return Err("native output exceeds bound".into());
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(log)?;
    // Do not log original jobspec environments. Callers retain that privately.
    if !args
        .iter()
        .any(|arg| arg == "jobspec" || arg == "--dry-run")
    {
        file.write_all(&output)?;
    }
    file.write_all(&errors)?;
    file.sync_all()?;
    Ok((status.and_then(|s| s.code()).unwrap_or(124), output))
}

pub fn run(
    inputs: &Inputs,
    options: &api::Options,
    name: &str,
    command: &[String],
    duration: u32,
) -> Result<Value> {
    let root = inputs.out.join(name);
    if !root.exists() {
        fs::create_dir(&root)?;
    }
    let result = root.join("job.json");
    if result.exists() {
        let value: Value = serde_json::from_slice(&durable::read(&result)?)?;
        evidence::verify_references(&value)?;
        if value["exit"] != 0
            || value["allocation_verified"] != true
            || value["cleanup_verified"] != true
        {
            return Err("retained admitted job did not pass".into());
        }
        return Ok(value);
    }
    let intent = json!({"name":format!("x3-{}-{name}",&durable::digest(inputs.out.as_os_str().as_encoded_bytes())[..20]),
        "command":command,"duration":duration,"cores":1,"memory_gib":2,"host":"dl380g10"});
    let intent_path = root.join("intent.json");
    let job_path = root.join("jobid");
    let native = root.join("native.log");
    let spec_path = root.join("jobspec.json");
    let observation_path = root.join("instance.json");
    if !observation_path.exists() {
        let endpoint = options.resolved_uri()?;
        let (start_exit, start) = capture(
            options,
            &["getattr".into(), "broker.starttime".into()],
            &native,
        )?;
        let (config_exit, config) = capture(options, &["config".into(), "get".into()], &native)?;
        if start_exit != 0 || config_exit != 0 || start.is_empty() || config.is_empty() {
            return Err("instance receipts unavailable before admission".into());
        }
        let config = durable::blob(&root.join("loaded-config.json"), &config)?;
        write(
            &observation_path,
            &json!({"endpoint":endpoint,"broker_starttime":String::from_utf8(start)?.trim(),
            "loaded_config_sha256":config.sha256,"loaded_config":config}),
        )?;
    }
    let id = if job_path.exists() {
        if serde_json::from_slice::<Value>(&durable::read(&intent_path)?)? != intent {
            return Err("native job inputs changed".into());
        }
        fs::read_to_string(&job_path)?.trim().to_owned()
    } else if intent_path.exists() {
        if serde_json::from_slice::<Value>(&durable::read(&intent_path)?)? != intent {
            return Err("native recovery inputs changed".into());
        }
        let expected: Value = serde_json::from_slice(&durable::read(&spec_path)?)?;
        let (exit, raw) = capture(
            options,
            &[
                "jobs".into(),
                "-a".into(),
                "--json".into(),
                format!("--name={}", intent["name"].as_str().ok_or("job name")?),
            ],
            &native,
        )?;
        if exit != 0 {
            return Err("native recovery query unavailable".into());
        }
        let jobs: Value = serde_json::from_slice(&raw)?;
        let jobs = jobs["jobs"]
            .as_array()
            .ok_or("native recovery candidates missing")?;
        if jobs.len() > 128 {
            return Err("too many native recovery candidates".into());
        }
        let mut matches = vec![];
        for job in jobs {
            if job["name"] != intent["name"] {
                continue;
            }
            let id = job["jobid"]
                .as_str()
                .or_else(|| job["id"].as_str())
                .map(String::from)
                .or_else(|| {
                    job["jobid"]
                        .as_u64()
                        .or_else(|| job["id"].as_u64())
                        .map(|n| n.to_string())
                })
                .ok_or("native candidate ID")?;
            let id = api::canonical_job_id(&id)?;
            if original(options, &id, &native)? != expected {
                return Err("conflicting original native jobspec; no adoption".into());
            }
            matches.push(id);
        }
        if matches.len() != 1 {
            return Err("native dispatch uncertain; never repeat submission".into());
        }
        let id = matches.remove(0);
        durable::atomic(&job_path, id.as_bytes())?;
        id
    } else {
        let mut args = vec![
            "submit".into(),
            "--dry-run".into(),
            "--env=-*".into(),
            "--rlimit=-*".into(),
            "-n1".into(),
            format!("-t{duration}s"),
            "--resources-shape=slot/[core=1;memory=2]".into(),
            "--requires=host:dl380g10".into(),
            format!("--job-name={}", intent["name"].as_str().ok_or("job name")?),
            format!("--cwd={}", inputs.repo.display()),
            "--input=/dev/null".into(),
            format!("--output={}", root.join("stdout").display()),
            format!("--error={}", root.join("stderr").display()),
        ];
        let mut environment = tl3_controller::production::flux_job_environment();
        environment.insert(
            "FLUX_SCRATCH".into(),
            "/home/rob/tl-gates/flux-validation".into(),
        );
        for key in [
            "OMP_NUM_THREADS",
            "MKL_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "TORCH_NUM_THREADS",
            "NUMEXPR_NUM_THREADS",
            "VECLIB_MAXIMUM_THREADS",
            "BLIS_NUM_THREADS",
        ] {
            environment.insert(key.into(), "1".into());
        }
        for (name, value) in environment {
            args.push(format!("--env={name}={value}"));
        }
        args.extend_from_slice(command);
        let (exit, raw) = capture(options, &args, &native)?;
        if exit != 0 {
            return Err("native jobspec rendering failed".into());
        }
        let _: Value = serde_json::from_slice(&raw)?;
        durable::atomic(&spec_path, &raw)?;
        write(&intent_path, &intent)?;
        let (exit, raw) = capture(
            options,
            &[
                "job".into(),
                "submit".into(),
                spec_path.to_string_lossy().into_owned(),
            ],
            &native,
        )?;
        if exit != 0 {
            return Err("native submission failed or uncertain; intent retained".into());
        }
        let id = api::canonical_job_id(String::from_utf8(raw)?.trim())?;
        if id.is_empty() || id.chars().any(char::is_whitespace) {
            return Err("invalid native job ID".into());
        }
        durable::atomic(&job_path, id.as_bytes())?;
        id
    };
    if original(options, &id, &native)?
        != serde_json::from_slice::<Value>(&durable::read(&spec_path)?)?
    {
        return Err("retained job original jobspec differs; no collection or cancellation".into());
    }
    // Every native job is fully collected before any following dispatch.
    loop {
        let (exit, _) = capture(
            options,
            &[
                "job".into(),
                "wait-event".into(),
                "--timeout=50s".into(),
                id.clone(),
                "clean".into(),
            ],
            &native,
        )?;
        if exit == 0 {
            break;
        }
        if exit != 1 && exit != 124 {
            return Err("native clean event unavailable; job retained".into());
        }
    }
    let (exit, status) = capture(
        options,
        &[
            "job".into(),
            "status".into(),
            "--json".into(),
            "--exception-exit-code=125".into(),
            id.clone(),
        ],
        &native,
    )?;
    let status_ref = durable::blob(&root.join("status.json"), &status)?;
    let _: Value = serde_json::from_slice(&status)?;
    let (events_exit, events) = capture(
        options,
        &[
            "job".into(),
            "eventlog".into(),
            "--format=json".into(),
            id.clone(),
        ],
        &native,
    )?;
    let events_ref = durable::blob(&root.join("eventlog.jsonl"), &events)?;
    let (r_exit, r) = capture(
        options,
        &["job".into(), "info".into(), id.clone(), "R".into()],
        &native,
    )?;
    let allocation = durable::blob(&root.join("R.json"), &r)?;
    let allocation_checked =
        r_exit == 0 && super::live::allocation(&serde_json::from_slice(&r)?, 1, "dl380g10").is_ok();
    let clean = events_exit == 0
        && events
            .split(|c| *c == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice::<Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .last()
            .is_some_and(|event| event["name"] == "clean");
    let absence = super::live::job_absence(inputs, options, &id, &root)?;
    let scratch = format!("/home/rob/tl-gates/flux-validation/{id}");
    let (scratch_exit, _) = capture(
        options,
        &[
            "exec".into(),
            "-r0".into(),
            "/bin/sh".into(),
            "-c".into(),
            "test -d /home/rob/tl-gates/flux-validation && test ! -e \"$1\"".into(),
            "x3-read-only".into(),
            scratch.clone(),
        ],
        &native,
    )?;
    let container_absent = if name == "preparation-job" {
        let container = format!(
            "x3-preflight-{}",
            &durable::digest(inputs.out.as_os_str().as_encoded_bytes())[..20]
        );
        let (exit, _) = capture(
            options,
            &[
                "exec".into(),
                "-r0".into(),
                "/usr/bin/podman".into(),
                "container".into(),
                "exists".into(),
                container,
            ],
            &native,
        )?;
        exit == 1
    } else {
        true
    };
    let cleanup_verified = clean && absence.0 && scratch_exit == 0 && container_absent;
    let mut logs = vec![];
    for name in ["stdout", "stderr"] {
        if root.join(name).exists() {
            logs.push(identity(&root.join(name))?);
        }
    }
    let value = json!({"id":id,"owner":"x3-qualification","intent":identity(&intent_path)?,"exit":exit,
        "jobspec":identity(&spec_path)?,"instance_receipts":[serde_json::from_slice::<Value>(&durable::read(&observation_path)?)?],
        "status":status_ref,"events":events_ref,"allocation_R":allocation,"allocation_verified":allocation_checked,
        "cleanup_verified":cleanup_verified,"cleanup":{"cgroup_absence":absence.1,"scratch":{"path":scratch,"absent":scratch_exit==0},"container_absent":container_absent},"logs":logs});
    durable::atomic(&result, &serde_json::to_vec_pretty(&value)?)?;
    if exit != 0 || !allocation_checked || !cleanup_verified {
        return Err("admitted preparation/verification job failed".into());
    }
    Ok(value)
}

fn original(options: &api::Options, id: &str, log: &Path) -> Result<Value> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == 'ƒ') {
        return Err("invalid native job ID".into());
    }
    let (exit, raw) = capture(
        options,
        &[
            "job".into(),
            "info".into(),
            "--original".into(),
            id.into(),
            "jobspec".into(),
        ],
        log,
    )?;
    if exit != 0 {
        return Err("original jobspec unavailable; no adoption".into());
    }
    Ok(serde_json::from_slice(&raw)?)
}

pub fn retained(inputs: &Inputs, name: &str) -> Option<Value> {
    let root = inputs.out.join(name);
    if !root.exists() {
        return None;
    }
    if let Ok(bytes) = durable::read(&root.join("job.json"))
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
    {
        return Some(value);
    }
    let reference = |name| identity(&root.join(name)).ok();
    Some(
        json!({"id":fs::read_to_string(root.join("jobid")).ok().map(|id|id.trim().to_owned()),
        "status":"UNKNOWN","reason":"incomplete_or_unreadable_native_collection","owner":"x3-qualification",
        "intent":reference("intent.json"),"jobspec":reference("jobspec.json"),
        "instance_receipts":reference("instance.json"),"events":reference("eventlog.jsonl"),
        "allocation_R":reference("R.json"),"native_log":reference("native.log")}),
    )
}
