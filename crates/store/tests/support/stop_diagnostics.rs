use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const TAIL_BYTES: u64 = 16 * 1024;
const COMMAND_BYTES: usize = 8 * 1024;

pub(super) fn failure(error: &postgresql_embedded::Error, data: &Path) -> String {
    // The embedded API flattens the failed command's stdout/stderr into this
    // error. Keep its labels/output; do not run another command for diagnostics.
    let command = error.to_string();
    let end = command.floor_char_boundary(COMMAND_BYTES.min(command.len()));
    let truncated = if end < command.len() {
        " [truncated]"
    } else {
        ""
    };
    let log = data.join("start.log");
    let tail = match tail(&log) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => format!("server log tail unavailable: {error}"),
    };
    let artifact = match retain(&log) {
        Ok(path) => format!("server log retained: {}", path.display()),
        Err(error) => format!("server log retention failed: {error}"),
    };
    format!(
        "DatabaseStopError: {}{truncated}\n{artifact}\nPostgres server log tail (at most {TAIL_BYTES} source bytes):\n{tail}",
        &command[..end]
    )
}

fn tail(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn retain(log: &Path) -> io::Result<PathBuf> {
    let root = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("BUILDBUDDY_ARTIFACTS_DIRECTORY"));
    let mut builder = tempfile::Builder::new();
    builder.prefix("postgres-stop-");
    let directory = match root {
        Some(root) => builder.tempdir_in(root)?,
        None => builder.tempdir()?,
    };
    // Only retain the server log, never the cluster or its password files.
    fs::copy(log, directory.path().join("server.log"))?;
    Ok(directory.keep().join("server.log"))
}
