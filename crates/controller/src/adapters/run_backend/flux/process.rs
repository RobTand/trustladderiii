use super::*;
use serde::{Deserialize, Serialize};
use std::process::{Command, ExitStatus};

pub(super) struct Observer {
    pub child: std::process::Child,
    stop: WatchStop,
    group: u32,
}
impl Observer {
    pub fn new(child: std::process::Child, stop: WatchStop) -> Self {
        let group = child.id();
        stop.register(group);
        Self { child, stop, group }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        kill_group(self.group);
        let _ = self.child.wait();
        self.stop.unregister(self.group);
    }
}

#[derive(Serialize)]
pub(super) struct ProcessStatus {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}
pub(super) struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub success: bool,
    pub status: ProcessStatus,
}
impl ProcessStatus {
    fn new(status: ExitStatus) -> Self {
        use std::os::unix::process::ExitStatusExt;
        Self { code: status.code(), signal: status.signal() }
    }
}
pub(super) fn bounded(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES { return Err(std::io::Error::other("process output exceeds bound")); }
    Ok(bytes)
}

impl Flux {
    fn ensure_generation(&self) -> Result<()> {
        let Some((run, expected)) = self.options.instance_generation.split_once('/') else { return Ok(()); };
        let output = Command::new("/usr/bin/systemctl").env_clear().envs(&self.options.environment)
            .args(["--user", "show", &format!("flux-vanilla-{run}.service"), "-p", "InvocationID", "--value"]).output()?;
        if !output.status.success() || output.stdout.len() > 128 || std::str::from_utf8(&output.stdout).ok().map(str::trim) != Some(expected) {
            return Err(BackendError::unknown("rank-zero systemd InvocationID unavailable or changed; retained IDs remain unknown"));
        }
        Ok(())
    }
    pub(super) fn command(&self) -> Command {
        let mut command = Command::new(&self.options.executable);
        command.args(&self.options.arguments).env_clear().envs(&self.options.environment).env("FLUX_URI", &self.options.uri);
        command
    }
    pub(super) fn capture(&self, args: &[&str]) -> Result<Captured> {
        self.capture_owned(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }
    pub(super) fn capture_owned(&self, args: &[String]) -> Result<Captured> {
        self.ensure_generation()?;
        let mut child = self.command().args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| BackendError::unknown("native stdout unavailable"))?;
        let stderr = child.stderr.take().ok_or_else(|| BackendError::unknown("native stderr unavailable"))?;
        let stdout = std::thread::spawn(move || bounded(stdout));
        let stderr = std::thread::spawn(move || bounded(stderr));
        let status = child.wait()?;
        let stdout = stdout.join().map_err(|_| BackendError::unknown("native stdout reader stopped"))??;
        let stderr = stderr.join().map_err(|_| BackendError::unknown("native stderr reader stopped"))??;
        Ok(Captured { stdout, stderr, success: status.success(), status: ProcessStatus::new(status) })
    }
    pub(super) fn job_environment(&self, record: &Record) -> Result<std::collections::BTreeMap<String, String>> {
        let mut env = record.request.environment.clone();
        let threads = record.request.reservation.native_threads.to_string();
        for name in ["OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS", "TORCH_NUM_THREADS", "NUMEXPR_NUM_THREADS", "VECLIB_MAXIMUM_THREADS", "BLIS_NUM_THREADS"] {
            env.insert(name.into(), threads.clone());
        }
        for (name, value) in [
            ("OUT", record.intent.output_dir.to_string_lossy().into_owned()),
            ("FLUX_SCRATCH", record.request.scratch_root.to_string_lossy().into_owned()),
            ("TL3_RUN_BACKEND", "flux".into()),
            ("TL3_RUN_INSTANCE", record.intent.instance.clone()),
            ("TL3_RUN_OWNER", record.intent.owner.clone()),
            ("TL3_RUN_CORRELATION", record.intent.correlation.clone()),
            ("TL3_RUN_REQUEST_DIGEST", record.intent.request_digest.clone()),
            ("TL3_EXECUTION_BACKEND", "flux".into()),
            ("TL3_RUN_BACKEND_CLI", self.options.bootstrap.cli.to_string_lossy().into_owned()),
            ("TL3_RUN_BACKEND_CONFIG", self.options.bootstrap.config.to_string_lossy().into_owned()),
        ] { env.insert(name.into(), value); }
        Ok(env)
    }
    pub(super) fn submit_args(&self, record: &Record) -> Result<Vec<String>> {
        let request = &record.request;
        let out = &record.intent.output_dir;
        let mut args = vec![
            "submit".into(), format!("--job-name={}", record.intent.correlation),
            format!("--cwd={}", request.tree.root.display()),
            "-n".into(), "1".into(), "-c".into(), request.reservation.cpus.to_string(),
            "-t".into(), format!("{}s", request.walltime_seconds),
            "--env=-*".into(), "--input=/dev/null".into(),
            format!("--output={}/stdout", out.display()), format!("--error={}/stderr", out.display()),
            format!("--setattr=user.delivery_key={}", record.intent.correlation),
            format!("--setattr=user.request_digest={}", record.intent.request_digest),
            format!("--setattr=user.tree_digest={}", request.tree.input_digest),
            format!("--setattr=user.runner_digest={}", request.runner.executable_digest),
            format!("--setattr=user.runtime_digest={}", request.runtime.manifest_digest),
            format!("--setattr=user.memory_bytes={}", request.reservation.memory_bytes),
        ];
        if request.reservation.gpus > 0 { args.extend(["-g".into(), request.reservation.gpus.to_string()]); }
        if let Some(dependency) = &request.runtime.host_dependency { args.push(format!("--requires=host:{}", dependency.host)); }
        for (key, value) in self.job_environment(record)? { args.push(format!("--env={key}={value}")); }
        args.push(request.runner.executable.to_string_lossy().into_owned());
        args.extend(request.runner.arguments.clone());
        Ok(args)
    }

    pub(super) fn matches(&self, record: &Record, spec: &Jobspec) -> Result<bool> {
        let mut command = vec![record.request.runner.executable.to_string_lossy().into_owned()];
        command.extend(record.request.runner.arguments.clone());
        Ok(spec.attributes.system.job.name == record.intent.correlation
            && spec.attributes.system.cwd == record.request.tree.root
            && spec.attributes.system.environment == self.job_environment(record)?
            && spec.tasks.len() == 1 && spec.tasks[0].command == command
            && spec.attributes.user.delivery_key == record.intent.correlation
            && spec.attributes.user.request_digest == record.intent.request_digest
            && spec.attributes.user.tree_digest == record.request.tree.input_digest
            && spec.attributes.user.runner_digest == record.request.runner.executable_digest
            && spec.attributes.user.runtime_digest == record.request.runtime.manifest_digest
            && spec.attributes.system.shell.options.output.stdout.path == record.intent.output_dir.join("stdout")
            && spec.attributes.system.shell.options.output.stderr.path == record.intent.output_dir.join("stderr"))
    }
}

#[derive(Deserialize)]
pub(super) struct Jobs { pub jobs: Vec<Job> }
#[derive(Deserialize)]
pub(super) struct Job { pub jobid: String, pub name: String }
#[derive(Deserialize)]
pub(super) struct Jobspec { pub attributes: Attributes, pub tasks: Vec<Task> }
#[derive(Deserialize)]
pub(super) struct Task { pub command: Vec<String> }
#[derive(Deserialize)]
pub(super) struct Attributes { pub system: System, pub user: User }
#[derive(Deserialize)]
pub(super) struct User {
    pub delivery_key: String, pub request_digest: String, pub tree_digest: String,
    pub runner_digest: String, pub runtime_digest: String,
}
#[derive(Deserialize)]
pub(super) struct System {
    pub cwd: std::path::PathBuf, pub environment: std::collections::BTreeMap<String, String>,
    pub job: JobName, pub shell: Shell,
}
#[derive(Deserialize)]
pub(super) struct JobName { pub name: String }
#[derive(Deserialize)]
pub(super) struct Shell { pub options: ShellOptions }
#[derive(Deserialize)]
pub(super) struct ShellOptions { pub output: OutputOptions }
#[derive(Deserialize)]
pub(super) struct OutputOptions { pub stdout: OutputPath, pub stderr: OutputPath }
#[derive(Deserialize)]
pub(super) struct OutputPath { pub path: PathBuf }

#[derive(Deserialize)]
struct NativeEvent {
    name: String,
    #[serde(default)]
    context: EventContext,
}
#[derive(Default, Deserialize)]
struct EventContext {
    status: Option<u32>,
    #[serde(rename = "type")]
    exception_type: Option<String>,
    severity: Option<u32>,
    note: Option<String>,
}
pub(super) fn parse_event(bytes: &[u8]) -> Result<NativeFact> {
    let event: NativeEvent = serde_json::from_slice(bytes)?;
    Ok(match event.name.as_str() {
        "start" => NativeFact::Start,
        "finish" => NativeFact::Finish { status: decode_wait_status(event.context.status.ok_or_else(|| BackendError::unknown("finish lacks wait status"))?) },
        "clean" => NativeFact::Clean,
        "exception" => NativeFact::Exception {
            exception_type: safe(&event.context.exception_type.ok_or_else(|| BackendError::unknown("exception lacks type"))?)?,
            severity: event.context.severity.ok_or_else(|| BackendError::unknown("exception lacks severity"))?,
            note: safe(&event.context.note.unwrap_or_default())?,
        },
        _ => NativeFact::Other { name: safe(&event.name)? },
    })
}
