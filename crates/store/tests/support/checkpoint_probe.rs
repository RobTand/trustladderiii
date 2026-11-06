//! Disposable-scratch I/O contention for diagnosing final checkpoint latency.
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Barrier, atomic::{AtomicBool, Ordering}},
    thread,
    time::{Duration, Instant},
};

pub struct Load {
    stop: Arc<AtomicBool>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl Load {
    pub fn start(root: &Path) -> Option<Self> {
        if !std::env::args().any(|arg| arg == "--fixture-checkpoint-io-probe") {
            return None;
        }
        let directory = root.join("checkpoint-io-probe");
        fs::create_dir(&directory).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for index in 0..2 {
            let path = directory.join(format!("owned-load-{index}"));
            let stop = stop.clone();
            let ready = ready.clone();
            workers.push(thread::spawn(move || {
                let mut file = OpenOptions::new().create_new(true).write(true).open(path).unwrap();
                file.set_len(32 * 1024 * 1024).unwrap();
                let block = vec![0x5a; 256 * 1024];
                let started = Instant::now();
                let mut operations = 0u64;
                ready.wait();
                while !stop.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(90) {
                    file.seek(SeekFrom::Start((operations % 128) * block.len() as u64)).unwrap();
                    file.write_all(&block).unwrap();
                    file.sync_all().unwrap();
                    operations += 1;
                }
                eprintln!("[fixture-checkpoint-io-probe] worker={index} fsyncs={operations} bytes={} elapsed={:?}", operations * block.len() as u64, started.elapsed());
            }));
        }
        ready.wait();
        eprintln!("[fixture-checkpoint-io-probe] started2owned-writers scratch_limit=64MiB buffers=512KiB duration_limit=90s");
        Some(Self { stop, workers })
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for worker in self.workers.drain(..) {
            worker.join().unwrap();
        }
    }
}
