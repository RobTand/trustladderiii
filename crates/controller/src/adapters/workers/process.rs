use super::source_error::at;
use crate::{Error, Result};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) fn read(path: &Path) -> Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = temporary(path);
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

/// A link publishes a complete claim or cancellation in one atomic operation.
/// A late launcher cannot cross a cancellation, even after a controller crash.
pub(super) fn cancel(path: &Path) -> Result<()> {
    if path.try_exists()? {
        return Ok(());
    }
    let temporary = temporary(path);
    write(&temporary, &json!({"cancelled": true}))?;
    match fs::hard_link(&temporary, path) {
        Ok(()) => File::open(path.parent().unwrap())?.sync_all()?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    fs::remove_file(temporary)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Identity {
    pid: i32,
    machine: String,
    start: u64,
    boot: String,
}

fn matches(identity: &Identity) -> Result<bool> {
    if fs::read_to_string("/etc/machine-id")?.trim() != identity.machine {
        return Err(Error::Invalid("worker record belongs to another machine"));
    }
    if fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim() != identity.boot {
        return Ok(false);
    }
    let Some(stat) = read_stat(File::open(format!("/proc/{}/stat", identity.pid)))? else {
        return Ok(false);
    };
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or(Error::Invalid("invalid process stat"))?
        .1
        .split_whitespace()
        .collect();
    let start = fields
        .get(19)
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or(Error::Invalid("invalid process start time"))?;
    Ok(start == identity.start && !matches!(fields.first(), Some(&"Z" | &"X")))
}

pub(super) fn alive(path: &Path) -> Result<bool> {
    match read(path)? {
        Some(value) if value["cancelled"] == true => Ok(false),
        Some(value) => matches(&serde_json::from_value(value)?),
        None => Ok(false),
    }
}

pub(super) fn pidfd(path: &Path) -> Result<Option<std::os::fd::OwnedFd>> {
    pidfd_with(path, |pid| pidfd_open(pid, PidfdFlags::empty()))
}

fn pidfd_with(
    path: &Path,
    open: impl FnOnce(Pid) -> rustix::io::Result<std::os::fd::OwnedFd>,
) -> Result<Option<std::os::fd::OwnedFd>> {
    let Some(value) = at("identity_read", read(path))? else {
        return Ok(None);
    };
    if value["cancelled"] == true {
        return Ok(None);
    }
    let identity: Identity = at("identity_read", serde_json::from_value(value))?;
    if !at("identity_read", matches(&identity))? {
        return Ok(None);
    }
    let pid = at(
        "identity_read",
        Pid::from_raw(identity.pid).ok_or(Error::Invalid("invalid worker pid")),
    )?;
    let fd = at(
        "pidfd_open",
        match open(pid) {
            Ok(fd) => Ok(fd),
            Err(rustix::io::Errno::SRCH) => return Ok(None),
            // Linux can reap the thread group between pidfd_open's PID lookup and
            // pidfd_prepare. Only a mismatched identity proves this EINVAL is exit.
            Err(rustix::io::Errno::INVAL) if !at("identity_read", matches(&identity))? => {
                return Ok(None);
            }
            Err(e) => Err(std::io::Error::from(e)),
        },
    )?;
    // Recheck after opening the pidfd, so PID reuse can never target another run.
    if !at("identity_read", matches(&identity))? {
        return Ok(None);
    }
    Ok(Some(fd))
}

pub(super) fn kill_and_wait(path: &Path) -> Result<()> {
    let Some(fd) = pidfd(path)? else {
        return Ok(());
    };
    match pidfd_send_signal(&fd, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(e) => return Err(std::io::Error::from(e).into()),
    }
    let mut fds = [rustix::event::PollFd::new(
        &fd,
        rustix::event::PollFlags::IN,
    )];
    loop {
        match rustix::event::poll(&mut fds, None) {
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(std::io::Error::from(e).into()),
        }
    }
}

/// The cgroup v2 scope a record's live process runs in, when that scope is its
/// own (#372): never the controller's cgroup, whose kill would stop tl3d.
pub(super) fn own_scope(path: &Path) -> Result<Option<PathBuf>> {
    let Some(value) = read(path)? else {
        return Ok(None);
    };
    if value["cancelled"] == true {
        return Ok(None);
    }
    let identity: Identity = serde_json::from_value(value)?;
    if !matches(&identity)? {
        return Ok(None);
    }
    let cgroup = match fs::read_to_string(format!("/proc/{}/cgroup", identity.pid)) {
        Ok(cgroup) => cgroup,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // Recheck, so a reused PID can never name another process's scope.
    if !matches(&identity)? {
        return Ok(None);
    }
    Ok(scope_of(&cgroup, &fs::read_to_string("/proc/self/cgroup")?))
}

fn scope_of(cgroup: &str, own: &str) -> Option<PathBuf> {
    let unified = |text: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix("0::"))
            .map(str::to_owned)
    };
    let path = unified(cgroup)?;
    (path.ends_with(".scope") && Some(&path) != unified(own).as_ref())
        .then(|| Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
}

/// Kill every process in a scope, including descendants a worker started.
pub(super) fn kill_scope(scope: &Path) -> Result<()> {
    match fs::write(scope.join("cgroup.kill"), "1") {
        Ok(()) => Ok(()),
        // The scope ended, and systemd collected it, before the kill.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn temporary(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn read_stat(opened: std::io::Result<File>) -> Result<Option<String>> {
    let mut file = match opened {
        Ok(file) => file,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let mut stat = String::new();
    match file.read_to_string(&mut stat) {
        Ok(_) => Ok(Some(stat)),
        // An open proc stat file returns ESRCH if the process exits before read.
        Err(error) if error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn source_pidfd_failure(
        errno: rustix::io::Errno,
        exit: bool,
    ) -> Result<Option<std::os::fd::OwnedFd>> {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("process.json");
        let mut child = Command::new("sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let stat = fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
        let start = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap();
        write(
            &path,
            &serde_json::json!({"pid":child.id(), "start":start.parse::<u64>().unwrap(),
            "machine":fs::read_to_string("/etc/machine-id").unwrap().trim(),
            "boot":fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim()}),
        )
        .unwrap();
        let result = pidfd_with(&path, |_| {
            if exit {
                drop(child.stdin.take());
                child.wait().unwrap();
            }
            Err(errno)
        });
        drop(child.stdin.take());
        child.wait().unwrap();
        result
    }

    #[test]
    fn source_pidfd_einval_after_exit_is_absence() {
        assert!(
            source_pidfd_failure(rustix::io::Errno::INVAL, true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn source_pidfd_einval_for_live_process_still_errors() {
        assert!(
            matches!(source_pidfd_failure(rustix::io::Errno::INVAL, false),
            Err(error) if matches!(super::super::source_error::cause(&error),
                Error::Io(io) if io.raw_os_error() == Some(22)))
        );
    }

    #[test]
    fn source_pidfd_esrch_remains_absence() {
        assert!(
            source_pidfd_failure(rustix::io::Errno::SRCH, true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn source_pidfd_other_errors_after_exit_still_error() {
        assert!(
            matches!(source_pidfd_failure(rustix::io::Errno::ACCESS, true),
            Err(error) if matches!(super::super::source_error::cause(&error),
                Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied))
        );
    }

    #[test]
    fn process_exit_between_stat_open_and_read_is_absence() {
        let mut child = Command::new("sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let stat = File::open(format!("/proc/{}/stat", child.id())).unwrap();
        drop(child.stdin.take());
        child.wait().unwrap();
        assert_eq!(read_stat(Ok(stat)).unwrap(), None);
    }

    #[test]
    fn process_exit_during_stat_open_is_absence() {
        let mut child = Command::new("sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let directory = File::open(format!("/proc/{}", child.id())).unwrap();
        drop(child.stdin.take());
        child.wait().unwrap();
        // Hold the directory resolved by path lookup, then finish the open after exit.
        let opened = rustix::fs::openat(
            &directory,
            "stat",
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        )
        .map(File::from)
        .map_err(std::io::Error::from);
        assert_eq!(
            opened.as_ref().unwrap_err().raw_os_error(),
            Some(rustix::io::Errno::SRCH.raw_os_error())
        );
        assert_eq!(read_stat(opened).unwrap(), None);
    }

    #[test]
    fn only_an_attempt_scope_of_its_own_is_killed() {
        let own = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tl3d.service\n";
        let attempt =
            "0::/user.slice/user-1000.slice/user@1000.service/tl3-workers.slice/run-r1.scope\n";
        assert_eq!(
            scope_of(attempt, own),
            Some(PathBuf::from(
                "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/tl3-workers.slice/run-r1.scope"
            ))
        );
        // Without a launcher the supervisor shares tl3d's cgroup; never kill it.
        assert_eq!(scope_of(own, own), None);
        let shared = "0::/user.slice/user-1000.slice/session-3.scope\n";
        assert_eq!(scope_of(shared, shared), None);
        assert_eq!(scope_of("12:pids:/legacy\n", own), None);
    }

    #[test]
    fn stat_open_permission_error_is_not_absence() {
        let denied = std::io::Error::from_raw_os_error(rustix::io::Errno::ACCESS.raw_os_error());
        assert!(matches!(read_stat(Err(denied)), Err(Error::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied));
    }
}
