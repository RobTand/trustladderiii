use rustix::{
    io::Errno,
    process::{Pid, geteuid, test_kill_process},
};
use std::{
    fs::{self, Metadata},
    io,
    os::unix::fs::MetadataExt,
    path::Path,
    time::Duration,
};

pub fn owned_metadata(path: &Path) -> io::Result<Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
    {
        return Err(io::Error::other("uncertain test fixture ownership"));
    }
    Ok(metadata)
}

fn dead_pid(path: &Path) -> io::Result<bool> {
    if !owned_metadata(path)?.is_file() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)?;
    let Some(pid) = text
        .lines()
        .next()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|pid| *pid > 0)
        .and_then(Pid::from_raw)
    else {
        return Ok(false);
    };
    Ok(test_kill_process(pid) == Err(Errno::SRCH))
}

fn abandoned(root: &Path) -> io::Result<bool> {
    let metadata = owned_metadata(root)?;
    if !metadata.is_dir()
        || metadata.modified()?.elapsed().unwrap_or_default() <= Duration::from_secs(3600)
    {
        return Ok(false);
    }
    // A fixture may still be initializing, or have temporarily stopped Postgres.
    let owner = root.join("fixture.pid");
    let owner_dead = match dead_pid(&owner) {
        Ok(true) => true,
        Ok(false) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let data = root.join("data");
    match owned_metadata(&data) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(owner_dead),
        _ => return Ok(false),
    }
    match dead_pid(&data.join("postmaster.pid")) {
        Ok(dead) => Ok(dead),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(owner_dead),
        Err(error) => Err(error),
    }
}

pub fn reap(temp_root: &Path) {
    let entries = match fs::read_dir(temp_root) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "cannot inspect test Postgres scratch {}: {error}",
                temp_root.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("tl3-store-pg-")
        {
            continue;
        }
        let root = entry.path();
        if abandoned(&root).unwrap_or(false) {
            match fs::remove_dir_all(&root) {
                Ok(()) => eprintln!("reaped abandoned test Postgres fixture {}", root.display()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => eprintln!(
                    "cannot reap test Postgres fixture {}: {error}",
                    root.display()
                ),
            }
        }
    }
}
