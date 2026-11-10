fn main() {
    if let Err(error) = cleanup_term_grace() {
        eprintln!("cannot preserve backend cleanup TERM grace: {error}");
        std::process::exit(1);
    }
    std::process::exit(tl3_controller::run_backend::cli::main());
}

fn cleanup_term_grace() -> std::io::Result<()> {
    // The in-job Python spawner blocks TERM across exec as its grace marker.
    // Only that route ignores TERM before unblocking it, so pending unit-stop
    // signals cannot kill cleanup, and helpers inherit the ignored disposition.
    // Other spawners retain their original TERM behavior. Deadlines use SIGKILL.
    // SAFETY: this binary entry runs before library code or threads start, and
    // every signal-set pointer refers to initialized, live stack storage.
    unsafe {
        let mut inherited: libc::sigset_t = std::mem::zeroed();
        if libc::sigprocmask(libc::SIG_SETMASK, std::ptr::null(), &mut inherited) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        match libc::sigismember(&inherited, libc::SIGTERM) {
            0 => return Ok(()),
            1 => {}
            _ => return Err(std::io::Error::last_os_error()),
        }
        if libc::signal(libc::SIGTERM, libc::SIG_IGN) == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error());
        }
        let mut signals: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(&mut signals) != 0
            || libc::sigaddset(&mut signals, libc::SIGTERM) != 0
            || libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}
