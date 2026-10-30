use super::*;
use crate::loom_tests::{bootstrap, delivery, model};
use crate::sync::{Arc, Mutex, mpsc};
use std::io::Write;

fn append(path: &std::path::Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn loom_worker_output_progress_wait_and_restart() {
    model(|settings| {
        let root = tempfile::tempdir().unwrap();
        let delivery = delivery();
        let runs = root.path().join("runs");
        fs::create_dir_all(runs.join(".ready")).unwrap();
        let run = run_root(&runs, &delivery.key);
        let attempt = run.join("attempt-00000000");
        fs::create_dir_all(&attempt).unwrap();
        let stat = fs::read_to_string("/proc/self/stat").unwrap();
        let start: u64 = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse()
            .unwrap();
        process::write(
            &attempt.join("claim.json"),
            &serde_json::json!({"pid":std::process::id(),
            "machine":fs::read_to_string("/etc/machine-id").unwrap().trim(), "start":start,
            "boot":fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim()}),
        )
        .unwrap();
        let controller = crate::Controller::open(
            Store::open(settings).unwrap(),
            crate::adapters::simulation(&root.path().join("fakes")).unwrap(),
            bootstrap(),
        )
        .unwrap();
        drop(controller);
        let mut store = Store::open(settings).unwrap();
        let snapshot = store.load().unwrap();
        let mut commit =
            tl3_store::Commit::new("watch-fixture", serde_json::json!(null), &snapshot);
        commit.rows.push(tl3_store::RowChange::Put {
            table: "watches".into(),
            id: delivery.key.clone(),
            value: serde_json::to_value(&delivery).unwrap(),
        });
        // worker_watches joins the outbox, as production startup does.
        commit.effects.push(tl3_store::NewEffect {
            key: delivery.key.clone(),
            kind: "Workers".into(),
            payload: serde_json::to_value(&delivery).unwrap(),
        });
        store.commit(&commit).unwrap();
        store.claim_worker(&delivery.key, "0", 0, 0).unwrap();
        let lock = Arc::new(Mutex::new(()));
        let writer_lock = lock.clone();
        let stdout = attempt.join("stdout.jsonl");
        let output = stdout.clone();
        let producer = crate::loom_tests::spawn(move || {
            {
                let _guard = writer_lock.lock().unwrap();
                append(&output, b"{\"event\":\"progress\",");
            }
            thread::yield_now();
            let _guard = writer_lock.lock().unwrap();
            append(&output, format!("\"unit\":\"model_turn\"}}\n{{\"event\":\"progress\",\"unit\":\"external_wait\",\"wait\":{{\"upstream\":\"prismabuild\",\"key\":\"{}\"}}}}\n{{\"event\":\"progress\",\"unit\":\"external_wait\",\"wait\":{{\"upstream\":\"prismabuild\",\"key\":\"{}\"}}}}\n{{\"event\":\"progress\",\"unit\":\"tool_result\"}}\n", "a".repeat(64), "a".repeat(64)).as_bytes());
        });
        let active = ActiveRuns::default();
        let mut source = WorkerOutputSource::new(runs.clone(), settings.clone(), active.clone());
        let (sender, receiver) = mpsc::channel();
        let reading = settings.clone();
        let consumer = crate::loom_tests::spawn(move || {
            let mut store = Store::open(&reading).unwrap();
            let mut inputs = vec![];
            for _ in 0..3 {
                let input = receiver.recv().unwrap();
                if let Input::Observation {
                    observation: Observation::Progress { checkpoint, .. },
                    ..
                } = &input
                {
                    let offset: i64 = checkpoint.split_once(':').unwrap().1.parse().unwrap();
                    assert!(
                        store.worker_progress(None).unwrap()[0].output_offset >= offset,
                        "progress published before persistence"
                    );
                }
                inputs.push(input);
            }
            (inputs, receiver)
        });
        for _ in 0..2 {
            let _guard = lock.lock().unwrap();
            source.poll_once(&mut store, &sender).unwrap();
        }
        producer.join().unwrap();
        source.poll_once(&mut store, &sender).unwrap();
        let (inputs, receiver) = consumer.join().unwrap();
        assert!(receiver.try_recv().is_err(), "source doubled progress");
        assert_eq!(inputs.len(), 3, "lost or duplicated progress/wait event");
        assert!(matches!(
            &inputs[0],
            Input::Observation {
                observation: Observation::Progress { .. },
                ..
            }
        ));
        assert!(matches!(&inputs[1], Input::Look {key} if key == &delivery.key));
        assert!(matches!(
            &inputs[2],
            Input::Observation {
                observation: Observation::Progress { .. },
                ..
            }
        ));
        let before = store.worker_progress(None).unwrap();
        assert_eq!(
            before[0].output_offset,
            fs::metadata(&stdout).unwrap().len() as i64
        );
        assert_eq!(before[0].unit, "tool_result");
        assert!(before[0].external_wait.is_none());
        let mut restarted = WorkerOutputSource::new(runs, settings.clone(), active);
        restarted.poll_once(&mut store, &sender).unwrap();
        assert!(receiver.try_recv().is_err(), "restart doubled progress");
        assert_eq!(
            store.worker_progress(None).unwrap(),
            before,
            "replay changed durable progress"
        );
    });
}
