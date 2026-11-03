//! #430: the worker appends each finished span to `trace.jsonl` before any
//! network call. When an attempt ends, the controller ships what the worker
//! couldn't, and closes the trace of a worker that died first. Retirement
//! keeps an unshipped log in `runs/.traces` until a flusher ships it.
use super::process::{self, read};
use crate::{Error, Result};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::UNIX_EPOCH,
};

pub(super) const LOG: &str = "trace.jsonl";
pub(super) const HELD: &str = ".traces";
pub(super) const WAKE: &str = ".wake";
/// Held logs of retired runs, at most. One span log is about 6 MiB at most.
pub(super) const HELD_BYTES: u64 = 256 << 20;
const CLAIM: &str = "trace.jsonl.flushing";
/// What a flusher needs from its attempt, moved together when a run retires.
const HELD_FILES: [&str; 5] = [LOG, CLAIM, "exit.json", "launch.json", "stopped.json"];

/// How flushers start: in the deploy's launcher scope, so a tl3d restart
/// leaves them running, and from the attempt's own release unless deploy has
/// removed it.
pub(super) struct Flusher<'a> {
    pub launcher: &'a [String],
    pub python: &'a Path,
}

fn shipped(directory: &Path) -> Result<bool> {
    Ok(directory.join(format!("{LOG}.shipped")).try_exists()?)
}

fn missing(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// Start the flusher for a directory holding an unshipped span log, unless a
/// live flusher already holds it. It reads the log from stdin, so retirement
/// may move or remove the log under it.
pub(super) fn start(directory: &Path, flusher: &Flusher) -> Result<Option<Child>> {
    if shipped(directory)? {
        return Ok(None);
    }
    let log = directory.join(LOG);
    let file = match File::open(&log) {
        Ok(file) => file,
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let claim = directory.join(CLAIM);
    if claim.try_exists()? {
        if process::alive(&claim)? {
            return Ok(None);
        }
        // Its flusher died; the next one claims the log afresh.
        match fs::remove_file(&claim) {
            Err(e) if !missing(&e) => return Err(e.into()),
            _ => {}
        }
    }
    let python = read(&directory.join("launch.json"))?
        .and_then(|launch| launch["python"].as_str().map(PathBuf::from))
        .filter(|python| python.exists())
        .unwrap_or_else(|| flusher.python.to_owned());
    let mut command = match flusher.launcher.split_first() {
        Some((launcher, args)) => {
            let mut command = Command::new(launcher);
            command.args(args).arg(&python);
            command
        }
        None => Command::new(&python),
    };
    Ok(Some(
        command
            .args(["-I", "-m", "tl3_workers.tracing", "flush"])
            .arg(&log)
            .stdin(file)
            .stdout(Stdio::null())
            .spawn()?,
    ))
}

/// The run's ending is the trigger. Tracing never delays or fails it.
pub(super) fn flush(directory: &Path, flusher: &Flusher) {
    match start(directory, flusher) {
        Ok(Some(child)) => reap(directory, child),
        Ok(None) => {}
        Err(error) => eprintln!(
            "trace flush for {} did not start: {error}",
            directory.display()
        ),
    }
}

/// Move an attempt's unshipped span log out of the run root before retirement
/// removes it. The model key and the rest of the attempt are not kept. Files
/// are staged in the attempt first, so the held directory appears complete in
/// one rename, and a retry after a crash finishes an interrupted hold.
pub(super) fn hold(attempt: &Path, runs: &Path) -> Result<()> {
    let staging = attempt.join(".held");
    if shipped(attempt)? || !(attempt.join(LOG).try_exists()? || staging.join(LOG).try_exists()?) {
        return Ok(());
    }
    let (Some(run), Some(name)) = (
        attempt.parent().and_then(Path::file_name),
        attempt.file_name(),
    ) else {
        return Err(Error::Invalid("span log outside a run root"));
    };
    fs::create_dir_all(&staging)?;
    for file in HELD_FILES {
        match fs::rename(attempt.join(file), staging.join(file)) {
            Err(e) if !missing(&e) => return Err(e.into()),
            _ => {}
        }
    }
    File::open(&staging)?.sync_all()?;
    let parent = runs.join(HELD);
    fs::create_dir_all(&parent)?;
    fs::rename(
        &staging,
        parent.join(format!(
            "{}-{}",
            run.to_string_lossy(),
            name.to_string_lossy()
        )),
    )?;
    File::open(&parent)?.sync_all()?;
    Ok(())
}

/// On an event (tl3d opening, attempts ending, a shipper reaching MLflow):
/// start flushers for the ended attempts and every held log, remove held logs
/// that shipped, and drop the oldest held logs beyond `cap` bytes. Returns how
/// many were dropped.
pub(super) fn sweep(runs: &Path, flusher: &Flusher, ended: &[PathBuf], cap: u64) -> Result<u64> {
    let mut held = Vec::new();
    match fs::read_dir(runs.join(HELD)) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let directory = entry.path();
                if shipped(&directory)? {
                    match fs::remove_dir_all(&directory) {
                        Err(e) if !missing(&e) => return Err(e.into()),
                        _ => {}
                    }
                    continue;
                }
                let mut bytes = 0;
                for file in fs::read_dir(&directory)? {
                    bytes += file?.metadata()?.len();
                }
                let modified = fs::metadata(directory.join(LOG))
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(UNIX_EPOCH);
                held.push((modified, bytes, directory));
            }
        }
        Err(e) if missing(&e) => {}
        Err(e) => return Err(e.into()),
    }
    held.sort();
    let mut total: u64 = held.iter().map(|(_, bytes, _)| bytes).sum();
    let mut evicted = 0;
    let mut kept = Vec::new();
    for (_, bytes, directory) in held {
        if total > cap {
            fs::remove_dir_all(&directory)?;
            total -= bytes;
            evicted += 1;
        } else {
            kept.push(directory);
        }
    }
    for directory in ended.iter().chain(&kept) {
        flush(directory, flusher);
    }
    Ok(evicted)
}

#[cfg(not(test))]
fn reap(_directory: &Path, child: Child) {
    super::reap(child);
}

/// Tests wait for the flusher a directory started instead of a reaper thread.
#[cfg(test)]
fn reap(directory: &Path, child: Child) {
    FLUSHES.lock().unwrap().push((directory.to_owned(), child));
}

#[cfg(test)]
pub(super) static FLUSHES: std::sync::Mutex<Vec<(PathBuf, Child)>> =
    std::sync::Mutex::new(Vec::new());

/// A rejected request has no worker attempt. Give its safe diagnostic the same
/// durable span-log/export path, without handing Python any rejected inputs.
pub(super) fn input_failure(
    settings: &tl3_config::Workers,
    key: &str,
    operation: serde_json::Value,
    mut tags: serde_json::Value,
    failure: &crate::worker_inputs::Failure,
) {
    use serde_json::json;
    use std::io::Write;
    let record = || -> Result<()> {
        let directory = super::run_root(&settings.runs.join(HELD), &format!("input-schema:{key}"));
        fs::create_dir_all(&directory)?;
        let log = directory.join(LOG);
        if log.try_exists()? || shipped(&directory)? {
            return Ok(());
        }
        tags["log"] = json!(log);
        let request = json!({"protocol":1,"role":operation["kind"],"operation":operation,
            "model":{"name":"not_started","effort":null},"inputs":failure,"trace":tags});
        let mut command = match settings.launcher.split_first() {
            Some((launcher, args)) => {
                let mut command = Command::new(launcher);
                command.args(args).arg(&settings.python);
                command
            }
            None => Command::new(&settings.python),
        };
        let mut child = command
            .args(["-I", "-m", "tl3_workers.tracing", "input-failure"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let written = child
            .stdin
            .take()
            .ok_or(Error::Invalid("trace stdin missing"))?
            .write_all(&serde_json::to_vec(&request)?);
        let status = child.wait()?;
        written?;
        if !status.success() {
            return Err(Error::Invalid("input failure trace helper failed"));
        }
        Ok(())
    };
    if let Err(error) = record() {
        // Observability must never replace the original contract failure.
        eprintln!("input failure trace could not be recorded: {error}");
    }
}
