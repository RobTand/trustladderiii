use super::*;
use io::{OTHER, SHA, Schedule, Step, compare};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};

fn event(run: &io::Run) -> &Value {
    &run.results[0]["event"]["Reply"]
}
fn schedule(step: Step) -> Schedule {
    Schedule {
        steps: vec![step],
        ..Schedule::default()
    }
}
fn arm<'a>(source: &'a str, variant: &str) -> &'a str {
    let start = source
        .find(&format!("            Observation::{variant} {{"))
        .unwrap();
    let end = source[start + 1..]
        .find("\n            Observation::")
        .map(|offset| start + 1 + offset + 1)
        .unwrap_or_else(|| {
            source[start..]
                .find("            _ => unreachable!")
                .unwrap()
                + start
        });
    &source[start..end]
}
fn head(source: &str) -> &str {
    let start = source
        .find("    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {")
        .unwrap();
    let end = source[start..].find("\n    }\n").unwrap() + start + 7;
    &source[start..end]
}
#[test]
fn pinned_real_old_and_current_arms_and_head() {
    let pins: Value = serde_json::from_str(include_str!("pins.json")).unwrap();
    assert_eq!(pins["base"], "c3e6fbbb3625490d324c487eb1469f6d4794dba2");
    let production = include_str!("../../src/lib.rs");
    for (name, variant, base, reference, current) in [
        (
            "conflict",
            "RebaseConflict",
            include_str!("conflict-base.rs.txt"),
            include_str!("conflict_reference.rs"),
            include_str!("conflict_current.rs"),
        ),
        (
            "rebased",
            "Rebased",
            include_str!("rebased-base.rs.txt"),
            include_str!("rebased_reference.rs"),
            include_str!("rebased_current.rs"),
        ),
    ] {
        assert_eq!(arm(reference, variant), base);
        assert_eq!(
            format!("{:x}", Sha256::digest(base)),
            pins["arms"][name]["reference"]
        );
        assert_eq!(arm(current, variant), arm(production, variant));
        assert_eq!(
            format!("{:x}", Sha256::digest(arm(current, variant))),
            pins["arms"][name]["current"]
        );
        assert_eq!(head(reference), include_str!("head-base.rs.txt"));
        assert_eq!(head(current), head(production));
        assert_eq!(
            format!("{:x}", Sha256::digest(head(reference))),
            pins["head"]
        );
        assert_eq!(head(reference), head(current));
    }
}
#[test]
fn mandatory_conflict_accepts_and_preserves_nonempty_files() {
    let s = Schedule::default();
    let run = compare(&s);
    assert_eq!(event(&run)["reply"], json!("RebaseNeedsChanges"));
    assert_eq!(
        run.persisted_rows["0"],
        json!({"head":SHA,"target":OTHER,"files":s.steps[0].files})
    );
    assert_eq!(run.calls[0]["op"], "row.push");
    assert_eq!(run.writes, 1); // Commit only; a conflict never interns its head/target.
    let empty = compare(&schedule(Step {
        files: vec![],
        ..Step::default()
    }));
    assert_eq!(empty.persisted_rows["0"]["files"], json!([]));
}
#[test]
fn mandatory_both_arms_reject_wrong_phase_or_generation_without_effects() {
    for conflict in [false, true] {
        for (phase, current, observed) in
            [(1, 7, 7), (2, 7, 7), (3, 7, 7), (0, 7, 8), (0, u64::MAX, 0)]
        {
            let run = compare(&schedule(Step {
                conflict,
                phase,
                current,
                observed,
                sha: "invalid".into(),
                ..Step::default()
            }));
            assert!(run.staged_rows.is_empty());
            assert!(run.results[0]["event"].is_null());
            assert_eq!(
                run.calls
                    .iter()
                    .map(|c| c["op"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["translation.result", "transaction.commit"]
            );
        }
    }
}
#[test]
fn mandatory_missing_node_errors_before_any_row_or_intern() {
    for conflict in [false, true] {
        for (present, node) in [(false, 0), (true, 1), (true, u64::MAX)] {
            let run = compare(&schedule(Step {
                conflict,
                present,
                node,
                ..Step::default()
            }));
            assert!(
                run.results[0]["error"]
                    .as_str()
                    .unwrap()
                    .contains("rebase observation has no request")
            );
            assert!(run.calls.is_empty());
        }
    }
}
#[test]
fn mandatory_rebased_clears_before_intern_and_preserves_head_and_generation() {
    for generation in [0, 7, u64::MAX] {
        for sha in [SHA.to_owned(), "a".repeat(64)] {
            let run = compare(&schedule(Step {
                conflict: false,
                sha: sha.clone(),
                current: generation,
                observed: generation,
                ..Step::default()
            }));
            assert_eq!(event(&run)["generation"], generation);
            assert_eq!(event(&run)["reply"]["RebaseClean"]["head"], 1);
            assert!(run.persisted_rows.is_empty());
            assert_eq!(run.persisted_intern[&("0".into(), sha)], 1);
            assert_eq!(
                run.calls
                    .iter()
                    .map(|c| c["op"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                [
                    "row.push",
                    "intern.select",
                    "intern.insert",
                    "intern.result",
                    "translation.result",
                    "transaction.commit"
                ]
            );
        }
    }
}
#[test]
fn mandatory_sha_errors_keep_delete_staged_and_persisted_rows_unchanged() {
    for sha in [
        "".into(),
        "invalid".into(),
        "A".repeat(40),
        "g".repeat(40),
        "é".repeat(20),
    ] {
        let s = schedule(Step {
            conflict: false,
            sha,
            ..Step::default()
        });
        let run = compare(&s);
        assert_eq!(run.staged_rows.len(), 1);
        assert_eq!(run.calls.len(), 1);
        assert!(run.results[0]["error"].is_string());
        assert_eq!(run.persisted_rows, s.initial_rows);
        assert!(run.persisted_intern.is_empty());
    }
}
#[test]
fn mandatory_existing_intern_and_absent_conflict_row() {
    let mut s = schedule(Step {
        conflict: false,
        ..Step::default()
    });
    s.initial_rows.clear();
    s.interned.insert(("0".into(), SHA.into()), 41);
    s.interned.insert(("1".into(), SHA.into()), 91);
    let run = compare(&s);
    assert_eq!(event(&run)["reply"]["RebaseClean"]["head"], 41);
    assert_eq!(run.writes, 1);
    assert_eq!(run.persisted_intern, s.interned);
    assert!(run.persisted_rows.is_empty());
}
#[test]
fn mandatory_store_and_commit_errors_roll_back_dictionary_and_rows() {
    for failures in [vec![true], vec![false, true]] {
        let mut s = schedule(Step {
            conflict: false,
            ..Step::default()
        });
        s.failures = failures;
        let run = compare(&s);
        assert!(run.results[0]["error"].is_string());
        assert!(run.persisted_intern.is_empty());
        assert_eq!(run.persisted_rows, s.initial_rows);
        assert_eq!(run.staged_rows.len(), 1);
    }
}
#[test]
fn mandatory_conflict_then_clean_then_conflict_sequence() {
    let mut s = Schedule::default();
    s.steps.push(Step {
        conflict: false,
        ..Step::default()
    });
    s.steps.push(Step {
        sha: OTHER.into(),
        files: vec!["new.rs".into()],
        ..Step::default()
    });
    s.steps.push(Step {
        conflict: false,
        ..Step::default()
    });
    let run = compare(&s);
    assert_eq!(run.results.len(), 4);
    assert_eq!(run.persisted_intern.len(), 1);
    assert!(run.persisted_rows.is_empty());
    assert_eq!(run.writes, 5); // Four commits and exactly one intern INSERT.
}
fn step_strategy() -> impl Strategy<Value = Step> {
    (
        any::<bool>(),
        any::<bool>(),
        0u8..4,
        any::<u64>(),
        any::<u64>(),
        0u8..6,
        0u8..5,
        prop::collection::vec("[a-z/. ]{0,40}", 0..6),
    )
        .prop_map(
            |(conflict, present, phase, current, observed, sha, node, files)| Step {
                conflict,
                present,
                phase,
                current,
                observed: if observed % 2 == 0 { current } else { observed },
                node: match node {
                    0..=2 => 0,
                    3 => 1,
                    _ => u64::MAX,
                },
                sha: match sha {
                    0 => "".into(),
                    1 => "A".repeat(40),
                    2 => "a".repeat(64),
                    3 => "g".repeat(40),
                    4 => OTHER.into(),
                    _ => SHA.into(),
                },
                files,
            },
        )
}
#[test]
fn fixed_seed_differential_schedules_and_sequences() {
    let strategy = (
        prop::collection::vec(step_strategy(), 1..9),
        any::<u8>(),
        prop::collection::vec(any::<bool>(), 0..16),
    );
    let mut runner = TestRunner::new(Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(0x47_2a_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/rebase_translate_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(&strategy, |(steps, flags, failures)| {
            let mut s = Schedule {
                steps,
                select_failures: failures.clone(),
                failures,
                ..Schedule::default()
            };
            if flags & 1 != 0 {
                s.interned.insert(("0".into(), SHA.into()), 7);
            }
            if flags & 2 != 0 {
                s.interned.insert(("1".into(), SHA.into()), 11);
            }
            if flags & 4 != 0 {
                s.initial_rows.clear();
            }
            compare(&s);
            Ok(())
        })
        .unwrap();
    eprintln!(
        "G2a: 512 fixed-seed schedules/sequences plus every before/after write cut; seed 0x472a2026"
    );
}

#[test]
fn mandatory_store_select_error_after_staged_delete() {
    let s = Schedule {
        steps: vec![Step {
            conflict: false,
            ..Step::default()
        }],
        select_failures: vec![true],
        ..Schedule::default()
    };
    let run = compare(&s);
    assert!(
        run.results[0]["error"]
            .as_str()
            .unwrap()
            .contains("injected intern select failure")
    );
    assert_eq!(run.calls[0]["op"], "row.push");
    assert_eq!(run.calls[1]["op"], "intern.select.error");
    assert_eq!(run.calls.len(), 2);
    assert_eq!(run.persisted_rows, s.initial_rows);
    assert_eq!(run.persisted_intern, s.interned);
}

#[test]
fn fixed_seed_accepted_sequences_preserve_intern_state_across_transactions() {
    let strategy = prop::collection::vec(
        (
            any::<bool>(),
            any::<bool>(),
            prop::collection::vec("[a-z/]{1,20}", 0..6),
        ),
        1..17,
    );
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x47_2b_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/rebase_translate_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(&strategy, |steps| {
            let s = Schedule {
                steps: steps
                    .into_iter()
                    .map(|(conflict, other, files)| Step {
                        conflict,
                        sha: if other { OTHER.into() } else { SHA.into() },
                        files,
                        ..Step::default()
                    })
                    .collect(),
                ..Schedule::default()
            };
            let run = compare(&s);
            prop_assert_eq!(run.results.len(), s.steps.len());
            Ok(())
        })
        .unwrap();
    eprintln!(
        "G2a: 128 accepted sequences of up to 16 observations and every write cut; seed 0x472b2026"
    );
}
