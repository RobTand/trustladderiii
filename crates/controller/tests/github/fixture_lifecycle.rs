//! Controlled retention of the real daemon's ownership backend during restart.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn connect(settings: &tl3_config::Store) -> postgres::Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect_timeout(Duration::from_secs(10))
        .connect(postgres::NoTls)
        .unwrap()
}

fn signal(pid: i32, signal: &str) {
    assert!(Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .expect("signal test-owned backend")
        .success());
}

pub(super) struct RetainedOwner {
    rejected: Arc<AtomicBool>,
    monitor: Option<thread::JoinHandle<()>>,
}

impl RetainedOwner {
    pub(super) fn start(settings: &tl3_config::Store) -> Option<Self> {
        let mut observer = connect(settings);
        let pid: i32 = observer.query_one("SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND classid::bigint = 1414280003 AND objid::bigint = 1330533458 AND objsubid = 1 AND granted AND database = (SELECT oid FROM pg_database WHERE datname = current_database())", &[]).unwrap().get(0);
        signal(pid, "-STOP");
        eprintln!("[fixture-owner-probe] retained original daemon backend {pid}");
        let rejected = Arc::new(AtomicBool::new(false));
        let failed = rejected.clone();
        let monitor = thread::spawn(move || {
            // Always resume the owned backend, including assertion unwinding.
            struct Resume(i32);
            impl Drop for Resume {
                fn drop(&mut self) {
                    signal(self.0, "-CONT");
                }
            }
            let _resume = Resume(pid);
            let started = Instant::now();
            loop {
                let waiting: bool = observer.query_one("SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND classid::bigint = 1414280003 AND objid::bigint = 1330533458 AND objsubid = 1 AND NOT granted AND database = (SELECT oid FROM pg_database WHERE datname = current_database()))", &[]).unwrap().get(0);
                if waiting || failed.load(Ordering::SeqCst) {
                    eprintln!("[fixture-owner-probe] release backend {pid}: barrier_waiter={waiting} successor_rejected={}", failed.load(Ordering::SeqCst));
                    return;
                }
                assert!(started.elapsed() < Duration::from_secs(30), "neither ownership barrier nor ownership refusal observed");
                // Probe sampling only. Release is driven by database/readiness evidence.
                thread::sleep(Duration::from_millis(5));
            }
        });
        Some(Self { rejected, monitor: Some(monitor) })
    }

    pub(super) fn rejection_signal(&self) -> Arc<AtomicBool> {
        self.rejected.clone()
    }
}

impl Drop for RetainedOwner {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.take() {
            monitor.join().unwrap();
        }
    }
}

pub(super) fn rejected(signal: &Option<Arc<AtomicBool>>, line: &str) {
    if line.contains("controller ownership is still held by a Postgres session") {
        if let Some(signal) = signal {
            signal.store(true, Ordering::SeqCst);
        }
    }
}

pub(super) fn stop_with_users() {
    let mut database = support::Database::start();
    let settings = database.create("stop_users_probe");
    let mut idle = connect(&settings);
    let mut active = connect(&settings);
    let active_pid: i32 = active.query_one("SELECT pg_backend_pid()", &[]).unwrap().get(0);
    let query = thread::spawn(move || active.batch_execute("SELECT pg_sleep(300)"));
    let started = Instant::now();
    loop {
        let sleeping: bool = idle.query_one("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND state = 'active' AND wait_event = 'PgSleep')", &[&active_pid]).unwrap().get(0);
        if sleeping {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "owned active query did not start");
        thread::sleep(Duration::from_millis(5));
    }
    database.stop();
    assert!(query.join().unwrap().is_err(), "fast shutdown must terminate active query");
    assert!(idle.simple_query("SELECT 1").is_err(), "idle connection must also be stale after fast shutdown");
    println!("test fixture_fast_stop_active_and_idle_users ... ok");
    println!("1 GitHub shutdown fixture scenarios passed; no skips");
}
