//! Public evidence uses #302's Redactor in the installed worker runtime.
use crate::{Error, Result};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

const FRAME_BYTES: usize = 1024 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const BRIDGE: &str = r#"
import json
import sys
from tl3_workers.tracing import Redactor

redactor = Redactor(None)
limit = 1024 * 1024
while raw := sys.stdin.buffer.readline(limit + 1):
    if len(raw) > limit or not raw.endswith(b'\n'):
        raise SystemExit(1)
    text = json.loads(raw)
    response = json.dumps(redactor.text(text, limit=len(text)), ensure_ascii=False).encode()
    if len(response) + 1 > limit:
        raise SystemExit(1)
    sys.stdout.buffer.write(response + b'\n')
    sys.stdout.buffer.flush()
    del raw, text, response
"#;

fn infra(error: impl Into<io::Error>) -> Error {
    Error::RedactionInfra(error.into())
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "redaction deadline expired"))
}

fn worker_python() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers/.venv/bin/python")
}

struct Redactor {
    child: Child,
    socket: BufReader<UnixStream>,
}

impl Redactor {
    fn start(python: &Path, script: &str) -> io::Result<Self> {
        // Pin the venv's directory like worker launches, retaining its interpreter symlink.
        let directory = python
            .parent()
            .ok_or_else(|| io::Error::other("missing worker runtime"))?;
        let name = python
            .file_name()
            .ok_or_else(|| io::Error::other("missing worker interpreter"))?;
        let python = directory.canonicalize()?.join(name);
        let (controller, worker) = UnixStream::pair()?;
        let mut command = Command::new(python);
        command.env_clear();
        // Bazel's existing worker launcher needs these runtime/runfiles locations.
        for key in [
            "PATH",
            "HOME",
            "TMPDIR",
            "TEST_TMPDIR",
            "TEST_SRCDIR",
            "TEST_WORKSPACE",
            "RUNFILES_DIR",
            "RUNFILES_MANIFEST_FILE",
            "JAVA_RUNFILES",
            "PYTHON_RUNFILES",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let child = command
            .args(["-I", "-u", "-c", script])
            .stdin(Stdio::from(OwnedFd::from(worker.try_clone()?)))
            .stdout(Stdio::from(OwnedFd::from(worker)))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            child,
            socket: BufReader::with_capacity(8192, controller),
        })
    }

    fn text(&mut self, text: &str, deadline: Instant) -> io::Result<String> {
        if text.len() > FRAME_BYTES {
            return Err(io::Error::other("redaction input exceeds frame limit"));
        }
        let mut frame = serde_json::to_vec(&text.replace('\0', ""))?;
        frame.push(b'\n');
        if frame.len() > FRAME_BYTES {
            return Err(io::Error::other("redaction input exceeds frame limit"));
        }
        let mut pending = frame.as_slice();
        while !pending.is_empty() {
            self.socket
                .get_ref()
                .set_write_timeout(Some(remaining(deadline)?))?;
            let sent = self.socket.get_mut().write(pending)?;
            if sent == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "redaction helper closed",
                ));
            }
            pending = &pending[sent..];
        }
        frame.clear();
        loop {
            self.socket
                .get_ref()
                .set_read_timeout(Some(remaining(deadline)?))?;
            let buffer = self.socket.fill_buf()?;
            if buffer.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "redaction helper closed",
                ));
            }
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(buffer.len(), |index| index + 1);
            if frame.len() + count > FRAME_BYTES {
                return Err(io::Error::other("redaction response exceeds frame limit"));
            }
            frame.extend_from_slice(&buffer[..count]);
            self.socket.consume(count);
            if newline.is_some() {
                return Ok(serde_json::from_slice(&frame)?);
            }
        }
    }
}

impl Drop for Redactor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Service {
    python: PathBuf,
    script: &'static str,
    process: Option<Redactor>,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            python: worker_python(),
            script: BRIDGE,
            process: None,
        }
    }
}

impl Service {
    fn text(&mut self, text: &str, deadline: Instant) -> io::Result<String> {
        remaining(deadline)?;
        if self.process.is_none() {
            self.process = Some(Redactor::start(&self.python, self.script)?);
        }
        let result = self
            .process
            .as_mut()
            .expect("started helper")
            .text(text, deadline);
        if result.is_err() {
            // This call errors; the next call starts a fresh helper.
            self.process = None;
        }
        result
    }
}

struct Shared {
    service: Mutex<Option<Service>>,
    available: Condvar,
}

impl Shared {
    fn with<T>(&self, run: impl FnOnce(&mut Service, Instant) -> io::Result<T>) -> Result<T> {
        let deadline = Instant::now() + CALL_TIMEOUT;
        let mut state = self
            .service
            .lock()
            .map_err(|_| infra(io::Error::other("redaction lock poisoned")))?;
        while state.is_none() {
            let (next, _) = self
                .available
                .wait_timeout(state, remaining(deadline).map_err(infra)?)
                .map_err(|_| infra(io::Error::other("redaction lock poisoned")))?;
            state = next;
        }
        let mut service = state.take().expect("available helper");
        drop(state);
        let result = run(&mut service, deadline).map_err(infra);
        let mut state = self
            .service
            .lock()
            .map_err(|_| infra(io::Error::other("redaction lock poisoned")))?;
        *state = Some(service);
        self.available.notify_one();
        result
    }
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        service: Mutex::new(Some(Service::default())),
        available: Condvar::new(),
    })
}

pub(crate) fn configure(python: &Path) -> Result<()> {
    shared().with(|service, _| {
        if service.python != python {
            service.process = None;
            service.python = python.to_owned();
        }
        Ok(())
    })
}

pub(crate) fn text(text: &str) -> Result<String> {
    #[cfg(test)]
    if let Some(result) = TEST_SERVICE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|service| service.text(text, Instant::now() + Duration::from_millis(250)))
    }) {
        return result.map_err(infra);
    }
    shared().with(|service, deadline| service.text(text, deadline))
}

#[cfg(test)]
thread_local! {
    static TEST_SERVICE: std::cell::RefCell<Option<Service>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum Failure {
    Missing,
    Crash,
    Timeout,
    Oversized,
}

#[cfg(test)]
pub(crate) fn with_failure<T>(failure: Failure, run: impl FnOnce() -> T) -> T {
    let mut service = Service::default();
    assert!(service.process.is_none(), "helper starts lazily");
    match failure {
        Failure::Missing => service.python = PathBuf::from("/nonexistent/tl3-redaction-python"),
        Failure::Crash => service.script = "raise SystemExit(1)",
        Failure::Timeout => service.script = "import threading; threading.Event().wait()",
        Failure::Oversized => {
            service.script = "import sys; sys.stdin.readline(); sys.stdout.write('x' * (1024 * 1024 + 1)); sys.stdout.flush()"
        }
    }
    TEST_SERVICE.with(|slot| {
        *slot.borrow_mut() = Some(service);
    });
    let result = run();
    TEST_SERVICE.with(|slot| {
        let mut service = slot.borrow_mut().take().unwrap();
        assert!(service.process.is_none(), "failed helper must be discarded");
        service.python = worker_python();
        service.script = BRIDGE;
        assert_eq!(
            service
                .text("check-1", Instant::now() + CALL_TIMEOUT)
                .unwrap(),
            "check-1"
        );
        let pid = service.process.as_ref().unwrap().child.id();
        assert_eq!(
            service
                .text("check-2", Instant::now() + CALL_TIMEOUT)
                .unwrap(),
            "check-2"
        );
        assert_eq!(
            service.process.as_ref().unwrap().child.id(),
            pid,
            "reuse the helper"
        );
    });
    result
}
