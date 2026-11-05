//! #826: captured stderr reaches the actual stored/public incident after redaction.
#[allow(dead_code)]
mod redaction_support;
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
};
use tl3_kernel::{EffectKind, Event, Reply};
use tl3_store::Store;

const KEY: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

struct StderrChecks(adapters::prismabuild::Fake, Value);
impl Executor for StderrChecks {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Output::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            }
        ) {
            return Ok(Completion::confirmed(vec![
                serde_json::from_value(json!({"CheckExecutionFailure": {
                    "node": d.node(), "generation": d.generation, "action_key": KEY,
                    "diagnostic": self.1
                }}))?,
                Observation::Kernel(Event::Reply {
                    node: d.node(),
                    generation: d.generation,
                    reply: Reply::RunFailed,
                }),
            ]));
        }
        self.0.execute(d)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.0.reconcile(d)
    }
}

fn drain(c: &mut Controller) {
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}

/// #826 review: line/total/raw clipping cannot disclose a label-stripped credential.
fn capture_to_incident(case: usize) {
    let capture = r#"
import json, os, sys, tempfile
from pathlib import Path
from tl3_harness.supervision import Supervised, ExecutionOutcome, MAX_STDERR_BYTES
secret = 'zE9!pR4#cV7$'
cause = 'container shim refused: pid limit reached'
payloads = [
    'old line\n' * 80 + cause + ' ghp_abcdefghijklmnopqrstuvwxyz1234567890',
    'password: ' + secret + ' ' + '.' * 499 + '\n' + cause,
    'password: ' + secret + '\n' + '.' * 505 + '\n' + '.' * 504 + '\n' + cause,
    'password: ' + 'v' * 40000 + '\n' + cause,
]
records = []
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / 'stderr.bin'
    for payload in payloads:
        path.write_text(payload)
        command = [sys.executable, '-I', '-B', '-c',
            'import os; os.write(2, open(' + repr(str(path)) + ', "rb").read()); os._exit(125)']
        with Supervised(command, None, ()) as process:
            assert list(process.records()) == []
            records.append(process.incomplete().operator_json())
process = Supervised([], None, ())
header = b'[passwords]\n'
process.capture_stderr(b'x' * MAX_STDERR_BYTES)
process.capture_stderr(header + b'.' * (MAX_STDERR_BYTES - len(header)))
process.capture_stderr(('\nvalue = ' + secret + '\n' + cause).encode())
records.append(process.failure(ExecutionOutcome.SETUP_FAILURE, 'container_start', 'incomplete_report').operator_json())
print(json.dumps(records))
"#;
    let output = redaction_support::python_command()
        .args(["-I", "-c", capture])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let diagnostics: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostics.len(), 5);
    let mut db = support::Database::start();
    for (index, diagnostic) in diagnostics
        .into_iter()
        .enumerate()
        .filter(|(index, _)| *index == case)
    {
        let root = tempfile::tempdir().unwrap();
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::PrismaBuild,
            StderrChecks(adapters::prismabuild::Fake::new(root.path()), diagnostic),
        );
        let mut c = Controller::open(
            Store::open(&db.create(&format!("stderr_incident_{index}"))).unwrap(),
            registry,
            Bootstrap {
                repos: vec![Repository {
                    owner: "owner".into(),
                    name: "sandbox".into(),
                    installation: 3,
                }],
                rob: 42,
                slots: 1,
                pool: 1,
                fail_fast: false,
                operations: None,
            },
        )
        .unwrap();
        c.start_release("release-1").unwrap();
        drain(&mut c);
        c.intake(LabelledIssue {
            delivery: "label".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "Fix a greeting".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
        drain(&mut c);
        let incidents: Vec<Value> = c
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| r.table == "incidents" && r.value["open"] == true)
            .map(|r| r.value)
            .collect();
        assert_eq!(incidents.len(), 1);
        let summary = incidents[0]["summary"].as_str().unwrap();
        assert!(summary.contains("pid limit reached"), "{summary}");
        assert!(summary.contains(KEY), "{summary}");
        for secret in [
            "ghp_abcdefghijklmnopqrstuvwxyz1234567890",
            "zE9!pR4#cV7$",
            &"v".repeat(128),
        ] {
            assert!(!summary.contains(secret), "{summary}");
        }
        assert!(c.world().unwrap().valid());
    }
    db.stop();
}

macro_rules! boundary {
    ($name:ident, $index:expr) => {
        /// #826: keep each capture/incident boundary below one admitted shard's budget.
        #[test]
        fn $name() {
            capture_to_incident($index);
        }
    };
}
boundary!(newest_cause, 0);
boundary!(line_boundary, 1);
boundary!(total_boundary, 2);
boundary!(raw_boundary, 3);
boundary!(raw_section_boundary, 4);
