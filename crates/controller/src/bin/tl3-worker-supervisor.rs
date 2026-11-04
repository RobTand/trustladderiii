//! Durable waitpid owner. The v1 files remain readable by Python-era controllers.
use serde::Deserialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("worker supervisor: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    let root = Path::new(
        args.get(1)
            .ok_or_else(|| io::Error::other("missing attempt directory"))?,
    );
    if args.len() == 3 && args[2] == "--worker" {
        if !publish(&root.join("process.json"), &identity()?, true)? {
            return Ok(());
        }
        publish(
            &root.join("worker-started.json"),
            &format!("{{\"started_ms\": {}}}", now_ms()?),
            false,
        )?;
        #[derive(Deserialize)]
        struct Launch {
            python: String,
        }
        let launch: Launch = serde_json::from_slice(&fs::read(root.join("launch.json"))?)?;
        let command: Vec<String> = serde_json::from_slice(&fs::read(root.join("command.json"))?)?;
        // exec preserves the process.json identity, even if the waitpid owner dies.
        return Err(Command::new(&launch.python).args(command).exec());
    }
    if !publish(&root.join("claim.json"), &identity()?, true)? {
        return Ok(());
    }
    // A controller may die before reading the handshake. Only attempt files
    // are needed after this point, and the launcher owns our independent scope.
    let _ = io::stdout().write_all(b"claimed\n");
    rustix::stdio::dup2_stdout(File::options().write(true).open("/dev/null")?)?;
    let request = File::open(root.join("request.json"))?;
    let output = File::create(root.join("stdout.jsonl"))?;
    let errors = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("stderr.log"))?;
    // The proc link pins this running image if a deploy replaces the helper
    // between our claim and the child exec.
    let mut command = Command::new("/proc/self/exe");
    command
        .arg(root)
        .arg("--worker")
        .stdin(request)
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(errors.try_clone()?));
    // SAFETY: this runs only in the forked child before exec and invokes one
    // syscall, with no allocation or locks. CLOEXEC preserves Rust's spawn-error
    // pipe until exec while closing every non-stdio descriptor at that boundary,
    // matching Python Popen(close_fds=True). Failure aborts spawn, never leaks.
    unsafe {
        command.pre_exec(|| {
            if libc::close_range(3, u32::MAX, libc::CLOSE_RANGE_CLOEXEC as libc::c_int) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut worker = command.spawn()?;
    let status = worker.wait()?;
    output.sync_all()?;
    errors.sync_all()?;
    let code = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0));
    publish(
        &root.join("exit.json"),
        &format!("{{\"code\": {code}, \"finished_ms\": {}}}", now_ms()?),
        false,
    )?;
    Ok(())
}

fn now_ms() -> io::Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis())
}

fn identity() -> io::Result<String> {
    let pid = std::process::id();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let start: u64 = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or_else(|| io::Error::other("invalid process stat"))?
        .parse()
        .map_err(io::Error::other)?;
    let machine = fs::read_to_string("/etc/machine-id")?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    // Python json.dump's field order and separators are part of the v1 protocol.
    Ok(format!(
        "{{\"pid\": {pid}, \"machine\": {}, \"start\": {start}, \"boot\": {}}}",
        serde_json::to_string(machine.trim())?,
        serde_json::to_string(boot.trim())?
    ))
}

fn publish(path: &Path, bytes: &str, exclusive: bool) -> io::Result<bool> {
    let mut random = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    // Keep the old UUID4 temporary-name shape for interrupted publications too.
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{suffix}"));
    let temporary = Path::new(&name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    file.write_all(bytes.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let result = (|| {
        if exclusive {
            match fs::hard_link(temporary, path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
                Err(error) => return Err(error),
            }
        } else {
            fs::rename(temporary, path)?;
        }
        File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("publication needs a directory"))?,
        )?
        .sync_all()?;
        Ok(true)
    })();
    match fs::remove_file(temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    result
}
