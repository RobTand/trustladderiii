//! #430: the worker appends each finished span to `trace.jsonl` before any
//! network call. When an attempt ends, the controller ships what the worker
//! couldn't, and closes the trace of a worker that died first.
use super::process::read;
use crate::Result;
use std::{
    fs::File,
    path::Path,
    process::{Child, Command, Stdio},
};

pub(super) const LOG: &str = "trace.jsonl";

/// Start the flusher for an ended attempt whose span log isn't marked shipped.
/// It reads the log from stdin, so retirement may remove the attempt under it.
pub(super) fn start_flush(attempt: &Path) -> Result<Option<Child>> {
    let log = attempt.join(LOG);
    if attempt.join(format!("{LOG}.shipped")).try_exists()? {
        return Ok(None);
    }
    let file = match File::open(&log) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // The release the attempt ran from; deploy keeps it while the run lives.
    let Some(python) = read(&attempt.join("launch.json"))?
        .and_then(|launch| launch["python"].as_str().map(str::to_owned))
    else {
        return Ok(None);
    };
    Ok(Some(
        Command::new(python)
            .args(["-I", "-m", "tl3_workers.tracing", "flush"])
            .arg(&log)
            .stdin(file)
            .stdout(Stdio::null())
            .spawn()?,
    ))
}

/// The run's ending is the trigger. Tracing never delays or fails it.
pub(super) fn flush(attempt: &Path) {
    match start_flush(attempt) {
        Ok(Some(child)) => reap(attempt, child),
        Ok(None) => {}
        Err(error) => eprintln!(
            "trace flush for {} did not start: {error}",
            attempt.display()
        ),
    }
}

#[cfg(not(test))]
fn reap(_attempt: &Path, child: Child) {
    super::reap(child);
}

/// Tests wait for the flusher an attempt started instead of a reaper thread.
#[cfg(test)]
fn reap(attempt: &Path, child: Child) {
    FLUSHES.lock().unwrap().push((attempt.to_owned(), child));
}

#[cfg(test)]
pub(super) static FLUSHES: std::sync::Mutex<Vec<(std::path::PathBuf, Child)>> =
    std::sync::Mutex::new(Vec::new());
