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
        Self {
            code: status.code(),
            signal: status.signal(),
        }
    }
}
pub(super) fn bounded(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(std::io::Error::other("process output exceeds bound"));
    }
    Ok(bytes)
}

impl Flux {
    pub(super) fn ensure_generation(&self) -> Result<()> {
        let Some((run, expected)) = self.options.instance_generation.split_once('/') else {
            return Ok(());
        };
        let mut command = Command::new("/usr/bin/systemctl");
        command.env_clear().envs(&self.options.environment).args([
            "--user",
            "show",
            &format!("flux-vanilla-{run}.service"),
            "-p",
            "InvocationID",
            "--value",
        ]);
        self.check_generation(command, expected)
    }

    fn check_generation(&self, command: Command, expected: &str) -> Result<()> {
        let output = self.capture_command(command, None)?;
        if !output.success
            || output.stdout.len() > 128
            || std::str::from_utf8(&output.stdout).ok().map(str::trim) != Some(expected)
        {
            return Err(BackendError::unknown(
                "rank-zero systemd InvocationID unavailable or changed; retained IDs remain unknown",
            ));
        }
        Ok(())
    }
    pub(super) fn command(&self) -> Command {
        let mut command = Command::new(&self.options.executable);
        command
            .args(&self.options.arguments)
            .env_clear()
            .envs(&self.options.environment)
            .env("FLUX_URI", &self.options.uri);
        command
    }
    pub(super) fn capture(&self, args: &[&str]) -> Result<Captured> {
        self.capture_owned(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }
    pub(super) fn capture_owned(&self, args: &[String]) -> Result<Captured> {
        self.ensure_generation()?;
        let mut command = self.command();
        command.args(args);
        self.capture_command(command, None)
    }

    pub(super) fn capture_command(
        &self,
        mut command: Command,
        input: Option<Vec<u8>>,
    ) -> Result<Captured> {
        use std::io::Write;
        use std::os::unix::process::CommandExt;
        use std::time::{Duration, Instant};
        if self.stop.is_stopped() {
            return Err(BackendError::unknown("local observation stopped"));
        }
        let child = command
            .process_group(0)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut observer = Observer::new(child, self.stop.clone());
        let stdin = observer.child.stdin.take();
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            if let (Some(mut pipe), Some(input)) = (stdin, input) {
                pipe.write_all(&input)?;
            }
            Ok(())
        });
        let stdout = observer
            .child
            .stdout
            .take()
            .ok_or_else(|| BackendError::unknown("native stdout unavailable"))?;
        let stderr = observer
            .child
            .stderr
            .take()
            .ok_or_else(|| BackendError::unknown("native stderr unavailable"))?;
        let stdout = std::thread::spawn(move || bounded(stdout));
        let stderr = std::thread::spawn(move || bounded(stderr));
        let deadline = Instant::now() + Duration::from_secs(NATIVE_COMMAND_TIMEOUT_SECONDS);
        let status = loop {
            if self.stop.is_stopped() || Instant::now() >= deadline {
                break None;
            }
            match observer.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break None,
            }
        };
        // Kill any local descendants still holding pipes, then reap before joining readers.
        drop(observer);
        let written = writer
            .join()
            .map_err(|_| BackendError::unknown("native stdin writer stopped"))?;
        let stdout = stdout
            .join()
            .map_err(|_| BackendError::unknown("native stdout reader stopped"))??;
        let stderr = stderr
            .join()
            .map_err(|_| BackendError::unknown("native stderr reader stopped"))??;
        written?;
        let status = status.ok_or_else(|| {
            BackendError::unknown(
                "local native subprocess stopped or deadline expired; execution remains unknown",
            )
        })?;
        Ok(Captured {
            stdout,
            stderr,
            success: status.success(),
            status: ProcessStatus::new(status),
        })
    }
    pub(super) fn job_environment(
        &self,
        record: &Record,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let mut env = record.request.environment.clone();
        let threads = record.request.reservation.native_threads.to_string();
        for name in [
            "OMP_NUM_THREADS",
            "MKL_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "TORCH_NUM_THREADS",
            "NUMEXPR_NUM_THREADS",
            "VECLIB_MAXIMUM_THREADS",
            "BLIS_NUM_THREADS",
        ] {
            env.insert(name.into(), threads.clone());
        }
        for (name, value) in [
            (
                "OUT",
                record.intent.output_dir.to_string_lossy().into_owned(),
            ),
            (
                "TL3_RUN_DIAGNOSTIC_DIR",
                record.intent.output_dir.to_string_lossy().into_owned(),
            ),
            (
                "FLUX_SCRATCH",
                record.request.scratch_root.to_string_lossy().into_owned(),
            ),
            ("TL3_RUN_BACKEND", "flux".into()),
            ("TL3_RUN_INSTANCE", record.intent.instance.clone()),
            ("TL3_RUN_OWNER", record.intent.owner.clone()),
            ("TL3_RUN_CORRELATION", record.intent.correlation.clone()),
            (
                "TL3_RUN_REQUEST_DIGEST",
                record.intent.request_digest.clone(),
            ),
            ("TL3_EXECUTION_BACKEND", "flux".into()),
            (
                "TL3_RUN_BACKEND_CLI",
                self.options.bootstrap.cli.to_string_lossy().into_owned(),
            ),
            (
                "TL3_RUN_BACKEND_CONFIG",
                self.options.bootstrap.config.to_string_lossy().into_owned(),
            ),
        ] {
            env.insert(name.into(), value);
        }
        Ok(env)
    }
    pub(super) fn submit_args(&self, record: &Record) -> Result<Vec<String>> {
        let request = &record.request;
        let out = &record.intent.output_dir;
        let mut args = vec![
            "submit".into(),
            format!("--job-name={}", record.intent.correlation),
            format!("--cwd={}", request.tree.root.display()),
            "-n".into(),
            "1".into(),
            "-N".into(),
            "1".into(),
            "-c".into(),
            request.reservation.cpus.to_string(),
            "-t".into(),
            format!("{}s", request.walltime_seconds),
            "--env=-*".into(),
            "--input=/dev/null".into(),
            format!("--output={}/stdout", out.display()),
            format!("--error={}/stderr", out.display()),
            format!("--setattr=user.delivery_key={}", record.intent.correlation),
            format!(
                "--setattr=user.request_digest={}",
                record.intent.request_digest
            ),
            format!("--setattr=user.tree_digest={}", request.tree.input_digest),
            format!(
                "--setattr=user.runner_digest={}",
                request.runner.executable_digest
            ),
            format!(
                "--setattr=user.runtime_digest={}",
                request.runtime.manifest_digest
            ),
            format!(
                "--setattr=user.memory_bytes={}",
                request.reservation.memory_bytes
            ),
        ];
        if request.reservation.gpus > 0 {
            args.extend(["-g".into(), request.reservation.gpus.to_string()]);
        }
        if let Some(dependency) = &request.runtime.host_dependency {
            args.push(format!("--requires=host:{}", dependency.host));
        }
        for (key, value) in self.job_environment(record)? {
            args.push(format!("--env={key}={value}"));
        }
        args.push(request.runner.executable.to_string_lossy().into_owned());
        args.extend(request.runner.arguments.clone());
        Ok(args)
    }

    pub(super) fn matches(&self, record: &Record, spec: &Jobspec) -> Result<bool> {
        let reference = record.jobspec.as_ref().ok_or_else(|| {
            BackendError::unknown("original jobspec was not retained before dispatch")
        })?;
        let retained: Jobspec =
            serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
        Ok(*spec == retained)
    }
}

#[derive(Deserialize)]
pub(super) struct Jobs {
    pub jobs: Vec<Job>,
}
#[derive(Deserialize)]
pub(super) struct Job {
    #[serde(deserialize_with = "native_job_id")]
    pub jobid: String,
    pub name: String,
    #[serde(default)]
    pub state: Option<String>,
}
fn native_job_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Text(String),
        Decimal(u64),
    }
    let text = match Wire::deserialize(deserializer)? {
        Wire::Text(text) => text,
        Wire::Decimal(number) => number.to_string(),
    };
    canonical_job_id(&text).map_err(serde::de::Error::custom)
}

/// Exact native original, including resources, placement constraints, duration and task shape.
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub(super) struct Jobspec(serde_json::Value);

impl Jobspec {
    /// Fluxion's standing graph accounts in GiB leaves under a single node's slot.
    /// This requests admission; sdexec's fixed caps do not follow this count.
    pub(super) fn with_memory(mut self, reservation: &Reservation) -> Result<Self> {
        let invalid = || {
            BackendError::invalid(
                "native jobspec must have one node and one task slot with the requested cores",
            )
        };
        let resources = self
            .0
            .get_mut("resources")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(invalid)?;
        if resources.len() != 1 || resources[0]["type"] != "node" || resources[0]["count"] != 1 {
            return Err(invalid());
        }
        let slots = resources[0]
            .get_mut("with")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(invalid)?;
        if slots.len() != 1 || slots[0]["type"] != "slot" || slots[0]["count"] != 1 {
            return Err(invalid());
        }
        let leaves = slots[0]
            .get_mut("with")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(invalid)?;
        if leaves.len() != 1
            || leaves[0]["type"] != "core"
            || leaves[0]["count"] != reservation.cpus.get()
            || reservation.gpus != 0
        {
            return Err(invalid());
        }
        leaves.push(serde_json::json!({"type": "memory", "count": reservation.memory_bytes.get().div_ceil(1_u64 << 30)}));
        Ok(self)
    }
}

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
        "finish" => NativeFact::Finish {
            status: decode_wait_status(
                event
                    .context
                    .status
                    .ok_or_else(|| BackendError::unknown("finish lacks wait status"))?,
            ),
        },
        "clean" => NativeFact::Clean,
        "exception" => NativeFact::Exception {
            exception_type: safe(
                &event
                    .context
                    .exception_type
                    .ok_or_else(|| BackendError::unknown("exception lacks type"))?,
            )?,
            severity: event
                .context
                .severity
                .ok_or_else(|| BackendError::unknown("exception lacks severity"))?,
            note: safe(&event.context.note.unwrap_or_default())?,
        },
        _ => NativeFact::Other {
            name: safe(&event.name)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_guard_process_stops_and_reaps_while_silent() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("systemctl");
        let marker = root.path().join("guard-pid");
        std::fs::write(
            &script,
            b"#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nsleep 300\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o555)).unwrap();
        let mut command = Command::new(&script);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .arg(&marker);
        let backend = Flux::new(Options {
            executable: "/bin/true".into(),
            arguments: vec![],
            environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            instance_generation: "fixture".into(),
            uri: "local:///fixture".into(),
            journal_root: root.path().join("journal"),
            bootstrap: HostBootstrap {
                cli: "/bin/true".into(),
                config: root.path().join("options.json"),
            },
        })
        .unwrap();
        let stop = backend.stop.clone();
        let watch = std::thread::spawn(move || backend.check_generation(command, &"a".repeat(32)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(marker).unwrap();
        let started = Instant::now();
        stop.stop();
        assert!(watch.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            !Path::new("/proc").join(pid).exists(),
            "generation guard was not reaped"
        );
    }
}
