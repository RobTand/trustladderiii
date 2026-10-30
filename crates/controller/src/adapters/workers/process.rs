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
    let pid = Pid::from_raw(identity.pid).ok_or(Error::Invalid("invalid worker pid"))?;
    let fd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => return Ok(None),
        Err(e) => return Err(std::io::Error::from(e).into()),
    };
    // Recheck after opening the pidfd, so PID reuse can never target another run.
    if !matches(&identity)? {
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
    fn stat_open_permission_error_is_not_absence() {
        let denied = std::io::Error::from_raw_os_error(rustix::io::Errno::ACCES.raw_os_error());
        assert!(matches!(read_stat(Err(denied)), Err(Error::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied));
    }
}
