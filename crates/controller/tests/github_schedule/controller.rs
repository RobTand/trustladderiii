use super::*;
use fixture::{IO, Shell, State};
use tl3_controller::executor::{Delivery, Output};
use tl3_kernel::{Cont, NodeId, Notice, Outcome, QKind, Question};

fn delivery(case: u8) -> Delivery {
    let mut delivery: Delivery = serde_json::from_value(json!({
        "key":"shell-schedule", "epoch":1,"generation":0,
        "output":{"Notice":{"node":0,"notice":{"Summary":"Stopped"}}},
        "head_sha":null,"repository":{"owner":"owner","name":"sandbox","installation":3},
        "issue":12,"pull_request":null,"paths":[],"attempt":1,"round":1
    }))
    .unwrap();
    let node = NodeId(0);
    delivery.output = match case % 13 {
        0 => Output::Notice {
            node,
            notice: Notice::Summary(Outcome::Stopped),
        },
        1 => Output::Notice {
            node,
            notice: Notice::Summary(Outcome::Merged),
        },
        2 => Output::Notice {
            node,
            notice: Notice::Summary(Outcome::AlreadyDone),
        },
        3 => Output::Notice {
            node,
            notice: Notice::Summary(Outcome::Duplicate),
        },
        4 => Output::Notice {
            node,
            notice: Notice::Summary(Outcome::NotActionable),
        },
        5 => Output::Notice {
            node,
            notice: Notice::Incident { needs_rob: true },
        },
        6 => Output::Effect {
            node,
            seq: 1,
            kind: tl3_kernel::EffectKind::OpenPr,
            head: None,
        },
        7 => Output::Notice {
            node,
            notice: Notice::FileReplacement,
        },
        8 => Output::Comment {
            node,
            kind: tl3_controller::executor::CommentKind::Clarify,
        },
        9 => Output::Comment {
            node,
            kind: tl3_controller::executor::CommentKind::ConsoleNote,
        },
        10 => Output::Comment {
            node,
            kind: tl3_controller::executor::CommentKind::Restart,
        },
        11 => Output::Notice {
            node,
            notice: Notice::MeasuresPosted,
        },
        _ => Output::Notice {
            node,
            notice: Notice::Ask { question: 1 },
        },
    };
    if case % 13 == 5 {
        delivery.question_context =
            Some(json!({"issue_title":"A request","request":"Request link","incident_id":"5.0"}));
    }
    if case % 13 == 12 {
        delivery.question = Some(Question {
            id: 1,
            kind: QKind::EffectFailed,
            cont: Cont::Nowhere,
            cost: 0,
            clarified: false,
        });
    }
    if case % 13 == 9 {
        delivery.steering = Some(steering::Task::ConsoleNote {
            text: "A note".into(),
        });
    }
    delivery
}
#[derive(Debug, PartialEq)]
struct ShellRun {
    calls: Vec<Value>,
    results: Vec<Value>,
    persisted: Vec<Vec<u8>>,
    writes: usize,
}
fn run(values: &[u16], case: u8, crash: Option<(usize, bool)>, old: bool) -> ShellRun {
    IO.with(|io| {
        *io.borrow_mut() = State {
            values: values.to_vec(),
            crash,
            ..State::default()
        }
    });
    let delivery = delivery(case);
    let mut before = Shell::<true>::new();
    let mut after = Shell::<false>::new();
    let mut results = vec![];
    let mut persisted = vec![];
    for operation in 0..4 {
        let result = match operation {
            0 | 2 => {
                let result = if old {
                    legacy_controller::reconcile(&mut before, &delivery)
                } else {
                    production_controller::reconcile(&mut after, &delivery)
                };
                result.map(|r| match r {
                    tl3_controller::executor::Reconciliation::Found(completion) => {
                        json!({"found":completion})
                    }
                    tl3_controller::executor::Reconciliation::Absent => json!("absent"),
                    tl3_controller::executor::Reconciliation::Pending => json!("pending"),
                })
            }
            1 => if old {
                legacy_controller::execute(&mut before, &delivery)
            } else {
                production_controller::execute(&mut after, &delivery)
            }
            .map(|c| json!(c)),
            _ => if old {
                legacy_controller::recover(&mut before)
            } else {
                production_controller::recover(&mut after)
            }
            .map(|c| json!(c)),
        };
        results.push(match result {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error":error.to_string()}),
        });
        let crashed = IO.with(|io| {
            let io = io.borrow();
            persisted.push(serde_json::to_vec(&io.files).unwrap());
            io.crashed
        });
        if crashed {
            break;
        }
    }
    IO.with(|io| {
        let io = io.borrow();
        ShellRun {
            calls: io.calls.clone(),
            results,
            persisted,
            writes: io.writes,
        }
    })
}
fn check(values: &[u16], case: u8) -> Result<(), TestCaseError> {
    let before = run(values, case, None, true);
    let after = run(values, case, None, false);
    prop_assert_eq!(&before, &after, "case {} values {:?}", case, values);
    for index in 0..before.writes {
        for landed in [false, true] {
            let before = run(values, case, Some((index, landed)), true);
            let after = run(values, case, Some((index, landed)), false);
            prop_assert_eq!(
                before,
                after,
                "case {} values {:?}, crash {} landed {}",
                case,
                values,
                index,
                landed
            );
        }
    }
    Ok(())
}
#[test]
fn controller_io_schedule() {
    // Varying per-call reads while preserving valid fence and issue results.
    check(&[0, 0, 0, 0, 0, 0, 0, 1, 0, 0], 11).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller-regressions.txt");
    std::fs::write(&path, include_str!("controller-regressions.txt")).unwrap();
    let path = Box::leak(path.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(730633),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(
            &(prop::collection::vec(any::<u16>(), 1..24), 0u8..26),
            |(values, case)| check(&values, case),
        )
        .unwrap();
}
#[test]
fn production_shell_copies_are_exact() {
    let source = include_str!("../../src/adapters/github.rs");
    let copy = include_str!("production_controller.rs");
    // Each complete top-level method must be the real production body.
    for chunk in copy.split("\n    fn ").skip(1) {
        let chunk = chunk.split_once("\n    }").unwrap().0;
        assert!(
            source.contains(&format!("    fn {chunk}\n    }}")),
            "stale fixture: {}",
            chunk.lines().next().unwrap()
        );
    }
}
