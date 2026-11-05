use super::*;
use io::{A1Inputs, Entry, Frame, Profile, ReadOverride, Row, Schedule, Version};

fn schedule() -> Schedule {
    Schedule {
        profile: Profile::Plain,
        initial: 65,
        frames: vec![],
        appears_at: usize::MAX,
        write_results: vec![],
        file_results: vec![],
        overrides: vec![ReadOverride {
            op: "github.events".into(),
            suffix: "#100".into(),
            nth: 0,
            frame: Frame {
                word: 1,
                elapsed_ms: 0,
            },
        }],
    }
}
fn row(table: &'static str, id: &str, version: i64, value: Value) -> Row {
    Row {
        table,
        id: id.into(),
        value,
        version: Version(version),
    }
}
/// #608/#760: version equality, stale/new resumes, missing rows/node and invalid
/// bindings all execute the separate pinned QueueFact methods inside reconciliation.
#[test]
fn mandatory_a1_scheduled_hold_versions() {
    let s = schedule();
    let mut comparisons = 0;
    for (observed, resumed, unresolved) in [
        (None, None, true),
        (Some(9), None, true),
        (None, Some(10), true),
        (Some(9), Some(8), true),
        (Some(9), Some(9), true),
        (Some(9), Some(10), false),
        (Some(i64::MAX), Some(i64::MAX), true),
    ] {
        let mut rows = vec![row("pull_requests", "0", 0, json!({"number":100}))];
        if let Some(v) = observed {
            rows.push(row(
                "queue_facts",
                "owner/sandbox/3/100/dequeued:2",
                v,
                json!({}),
            ));
        }
        if let Some(v) = resumed {
            rows.push(row("queue_resumes", "0", v, json!({})));
        }
        // Later duplicate rows must never replace the first matching revision.
        if resumed.is_some() {
            rows.push(row("queue_resumes", "0", i64::MAX, json!({})));
        }
        io::with_inputs(
            A1Inputs {
                rows,
                ..A1Inputs::default()
            },
            || {
                comparisons += io::compare(&s, Entry::Reconcile).unwrap();
                io::require_count(&s, Entry::Reconcile, "projection.node", 1);
                if unresolved {
                    io::require_ack(&s, Entry::Reconcile, "Confirmed");
                } else {
                    io::require_count(&s, Entry::Reconcile, "github.issue", 1);
                }
            },
        );
    }
    io::with_inputs(A1Inputs::default(), || {
        comparisons += io::compare(&s, Entry::Reconcile).unwrap();
        io::require_count(&s, Entry::Reconcile, "projection.node", 1);
        io::require_ack(&s, Entry::Reconcile, "Confirmed");
    });
    io::with_inputs(
        A1Inputs {
            rows: vec![row("pull_requests", "bad", 0, json!({"number":100}))],
            ..A1Inputs::default()
        },
        || {
            comparisons += io::compare(&s, Entry::Reconcile).unwrap();
            // reconcile turns a non-GitHub error into Pending on both sides.
            io::require_count(&s, Entry::Reconcile, "projection.node", 1);
            io::require_count(&s, Entry::Reconcile, "github.issue", 0);
        },
    );
    eprintln!("A1 r1 scheduled holds: 9 cases; {comparisons} complete/crash comparisons");
}

/// Q52/#447/#760: nonempty proof is rendered by real publication(), then checked
/// inside both Read shells. Missing proof rejects before heal/queue-facts I/O.
#[test]
fn mandatory_a1_recorded_proof_present_and_missing() {
    let mut delivery = io::fixture_delivery();
    delivery.request_text = Some("Record the validation.".into());
    delivery.evidence = Some(tl3_controller::evidence::Evidence {
        checks: vec![json!({"head_sha":"head","report":true,"pass":true,
            "results":[{"kind":"regression","verdict":"pass","tests":[{"head":"passed"}]}]})],
        ..Default::default()
    });
    let proof =
        evidence::publication(&delivery, delivery.request_text.as_deref().unwrap()).unwrap();
    assert!(!proof.is_empty());
    let s = Schedule {
        initial: 0,
        overrides: vec![],
        ..schedule()
    };
    for present in [false, true] {
        let body = if present {
            format!("Change type: fix\n{proof}")
        } else {
            "Change type: fix".into()
        };
        io::with_inputs(
            A1Inputs {
                delivery: Some(delivery.clone()),
                body: Some(body),
                ..A1Inputs::default()
            },
            || {
                let comparisons = io::compare(&s, Entry::Read).unwrap();
                io::require_count(&s, Entry::Read, "github.pull", 2);
                if present {
                    io::require_count(&s, Entry::Read, "file.read", 1);
                    io::require_ops(&s, Entry::Read, &["github.requirements"]);
                } else {
                    io::require_field(
                        &s,
                        Entry::Read,
                        "error",
                        json!("Required recorded proof is missing from the PR body"),
                    );
                    for op in [
                        "file.read",
                        "github.requirements",
                        "github.events",
                        "github.issue",
                        "github.patch_body",
                    ] {
                        io::require_count(&s, Entry::Read, op, 0);
                    }
                    io::require_no_writes(&s, Entry::Read);
                }
                eprintln!(
                    "A1 r1 proof present={present}: {comparisons} complete/crash comparisons"
                );
            },
        );
    }
}

/// #760: scheduled hold calls compile the real old/current body, and no changed
/// A1 function resolves through the former shared unresolved_hold shim.
#[test]
fn mandatory_a1_scheduled_hold_source() {
    fn body<'a>(s: &'a str, name: &str) -> &'a str {
        let start = s.find(&format!("    fn {name}(")).unwrap();
        &s[start..start + s[start..].find("\n    }\n").unwrap() + 7]
    }
    for (source, copy) in [
        (
            include_str!("base-adapter.rs.txt"),
            include_str!("reference.rs"),
        ),
        (
            include_str!("../../src/adapters/mergify.rs"),
            include_str!("current.rs"),
        ),
    ] {
        assert_eq!(
            body(source, "unresolved_hold"),
            body(copy, "unresolved_hold")
        );
        let implementation = copy.find("impl ScheduledHold for QueueFact").unwrap();
        let method = copy.find("    fn unresolved_hold(").unwrap();
        assert!(method > implementation);
    }
    assert!(!include_str!("mod.rs").contains("Ok(snapshot.held())"));
}
