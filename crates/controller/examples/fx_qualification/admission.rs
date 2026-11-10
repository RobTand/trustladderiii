//! Serial non-backend preparation jobs, with durable native ownership.
use super::*;
use std::{io::{Read, Write}, process::{Command, Stdio}, time::{Duration, Instant}};

pub fn options(inputs: &Inputs, binary_root: &Path) -> Result<api::Options> {
    let instance = Path::new("/mnt/shared/flux-migration/INSTANCE");
    let locator = fs::read_to_string(instance)?;
    let fields: Vec<_> = locator.split_whitespace().collect();
    if fields.len() != 2 || !api::absolute(Path::new(fields[0])) { return Err("invalid INSTANCE locator".into()); }
    let prefix = Path::new(fields[0]);
    let mut environment = tl3_controller::production::flux_job_environment();
    environment.insert("PATH".into(),format!("{}/install/bin:{}/deps/bin:/usr/bin:/bin",prefix.display(),prefix.display()));
    environment.insert("LD_LIBRARY_PATH".into(),format!("{}/install/lib:{}/deps/lib",prefix.display(),prefix.display()));
    let journal = inputs.out.join("journal");
    fs::create_dir_all(&journal)?;
    let options = api::Options { executable:prefix.join("install/bin/flux"),arguments:vec![],environment,
        instance_file:instance.into(),journal_root:journal.clone(),bootstrap:api::HostBootstrap { cli:binary_root.join("tl3-run-backend"),config:journal.join("options.json") } };
    durable::atomic(&options.bootstrap.config,&serde_json::to_vec(&options)?)?;
    Ok(options)
}

pub fn capture(options: &api::Options, args: &[String], log: &Path) -> Result<(i32,Vec<u8>)> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(&options.executable).args(&options.arguments).args(args)
        .env_clear().envs(&options.environment).env("FLUX_URI",options.resolved_uri()?)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn()?;
    let stdout = child.stdout.take().ok_or("native stdout")?;
    let stderr = child.stderr.take().ok_or("native stderr")?;
    let reader = |stream: Box<dyn Read+Send>| std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut raw=Vec::new(); stream.take(8*1024*1024+1).read_to_end(&mut raw)?; Ok(raw)
    });
    let stdout = reader(Box::new(stdout));
    let stderr = reader(Box::new(stderr));
    let deadline = Instant::now()+Duration::from_secs(55);
    let status = loop {
        if let Some(status)=child.try_wait()? { break Some(status); }
        if Instant::now()>=deadline {
            // Observation process only. Never signal the native job.
            unsafe { libc::kill(-(child.id() as i32),libc::SIGKILL); }
            child.wait()?;
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output=stdout.join().map_err(|_|"stdout reader failed")??;
    let errors=stderr.join().map_err(|_|"stderr reader failed")??;
    if output.len()>8*1024*1024 || errors.len()>8*1024*1024 { return Err("native output exceeds bound".into()); }
    let mut file=fs::OpenOptions::new().create(true).append(true).open(log)?;
    // Do not log original jobspec environments. Callers retain that privately.
    if !args.iter().any(|arg|arg=="jobspec") { file.write_all(&output)?; }
    file.write_all(&errors)?; file.sync_all()?;
    Ok((status.and_then(|s|s.code()).unwrap_or(124),output))
}

pub fn run(inputs: &Inputs, options: &api::Options, name: &str, command: &[String], duration: u32) -> Result<Value> {
    let root=inputs.out.join(name);
    if !root.exists() { fs::create_dir(&root)?; }
    let result=root.join("job.json");
    if result.exists() {
        let value:Value=serde_json::from_slice(&durable::read(&result)?)?;
        evidence::verify_references(&value)?; return Ok(value);
    }
    let intent=json!({"name":format!("x3-{}-{name}",&durable::digest(inputs.out.as_os_str().as_encoded_bytes())[..20]),
        "command":command,"duration":duration,"cores":1,"memory_gib":2,"host":"dl380g10"});
    let intent_path=root.join("intent.json");
    let job_path=root.join("jobid");
    let native=root.join("native.log");
    let id=if job_path.exists() {
        if serde_json::from_slice::<Value>(&durable::read(&intent_path)?)?!=intent { return Err("native job inputs changed".into()); }
        fs::read_to_string(&job_path)?.trim().to_owned()
    } else if intent_path.exists() {
        return Err("native dispatch uncertain; do not repeat submission".into());
    } else {
        write(&intent_path,&intent)?;
        let mut args=vec!["submit".into(),"--env=-*".into(),"--rlimit=-*".into(),"-n1".into(),
            format!("-t{duration}s"),"--resources-shape=slot/[core=1;memory=2]".into(),"--requires=host:dl380g10".into(),
            format!("--job-name={}",intent["name"].as_str().ok_or("job name")?),
            format!("--cwd={}",inputs.repo.display()),"--input=/dev/null".into(),
            format!("--output={}",root.join("stdout").display()),format!("--error={}",root.join("stderr").display())];
        let mut environment=tl3_controller::production::flux_job_environment();
        environment.insert("FLUX_SCRATCH".into(),"/home/rob/tl-gates/flux-validation".into());
        for key in ["OMP_NUM_THREADS","MKL_NUM_THREADS","OPENBLAS_NUM_THREADS","TORCH_NUM_THREADS","NUMEXPR_NUM_THREADS","VECLIB_MAXIMUM_THREADS","BLIS_NUM_THREADS"] {
            environment.insert(key.into(),"1".into());
        }
        for (name,value) in environment { args.push(format!("--env={name}={value}")); }
        args.extend_from_slice(command);
        let (exit,raw)=capture(options,&args,&native)?;
        if exit!=0 { return Err("native submission failed or uncertain; intent retained".into()); }
        let id=String::from_utf8(raw)?.trim().to_owned();
        if id.is_empty() || id.chars().any(char::is_whitespace) { return Err("invalid native job ID".into()); }
        durable::atomic(&job_path,id.as_bytes())?;
        id
    };
    // Every native job is fully collected before any following dispatch.
    loop {
        let (exit,_)=capture(options,&["job".into(),"wait-event".into(),"--timeout=50s".into(),id.clone(),"clean".into()],&native)?;
        if exit==0 { break; }
        if exit!=1 && exit!=124 { return Err("native clean event unavailable; job retained".into()); }
    }
    let (exit,status)=capture(options,&["job".into(),"status".into(),"--json".into(),"--exception-exit-code=125".into(),id.clone()],&native)?;
    let status_ref=durable::blob(&root.join("status.json"),&status)?;
    let (_,events)=capture(options,&["job".into(),"eventlog".into(),"--format=json".into(),id.clone()],&native)?;
    let events_ref=durable::blob(&root.join("eventlog.jsonl"),&events)?;
    let (r_exit,r)=capture(options,&["job".into(),"info".into(),id.clone(),"R".into()],&native)?;
    let allocation=durable::blob(&root.join("R.json"),&r)?;
    let allocation_checked=r_exit==0 && super::live::allocation(&serde_json::from_slice(&r)?,1,"dl380g10").is_ok();
    let mut logs=vec![];
    for name in ["stdout","stderr"] { if root.join(name).exists() { logs.push(identity(&root.join(name))?); } }
    let value=json!({"id":id,"owner":"x3-qualification","intent":identity(&intent_path)?,"exit":exit,
        "status":status_ref,"events":events_ref,"allocation_R":allocation,"allocation_verified":allocation_checked,"logs":logs});
    durable::atomic(&result,&serde_json::to_vec_pretty(&value)?)?;
    if exit!=0 || !allocation_checked { return Err("admitted preparation/verification job failed".into()); }
    Ok(value)
}
