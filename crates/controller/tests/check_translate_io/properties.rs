use super::*;
use io::{OTHER, Read, Run, SHA, Schedule, Step, check_run, compare};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};
/// Copies the committed regressions to writable test output so proptest can
/// append new minimized failures there instead of the read-only runfiles tree.
fn writable_regressions(committed: &str, name: &str) -> &'static str {
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("TEST_TMPDIR"))
        .expect("writable test output");
    let path = std::path::PathBuf::from(output).join(name);
    std::fs::write(&path, committed).unwrap();
    Box::leak(path.to_str().unwrap().to_owned().into_boxed_str())
}
fn arm<'a>(s: &'a str, name: &str) -> &'a str {
    let a = s
        .find(&format!("            Observation::{name} {{"))
        .unwrap();
    let end = s[a + 1..]
        .find("\n            Observation::")
        .map(|o| a + o + 2)
        .unwrap_or_else(|| a + s[a..].find("            _ => unreachable!").unwrap());
    &s[a..end]
}
fn function<'a>(s: &'a str, name: &str) -> &'a str {
    let marker = if name == "decode_stored" {
        "decode_stored<"
    } else {
        name
    };
    let a = s.find(&format!("fn {marker}")).unwrap();
    let a = s[..a].rfind('\n').map(|a| a + 1).unwrap_or(0);
    let suffix = if s[a..].starts_with("    ") {
        "\n    }\n"
    } else {
        "\n}\n"
    };
    let end = a + s[a..].find(suffix).unwrap() + suffix.len();
    &s[a..end]
}
#[test]
fn pinned_real_arms_and_helper_closures() {
    let pins: Value = serde_json::from_str(include_str!("pins.json")).unwrap();
    assert_eq!(pins["base"], "751c00acfcfce94f6cebf62447776e0a44b3e520");
    let production = include_str!("../../src/lib.rs");
    for (name, variant, base, old, new) in [
        (
            "prepared",
            "WorktreePrepared",
            include_str!("prepared-base.rs.txt"),
            include_str!("prepared_reference.rs"),
            include_str!("prepared_current.rs"),
        ),
        (
            "plan",
            "CheckPlan",
            include_str!("plan-base.rs.txt"),
            include_str!("plan_reference.rs"),
            include_str!("plan_current.rs"),
        ),
        (
            "checks",
            "Checks",
            include_str!("checks-base.rs.txt"),
            include_str!("checks_reference.rs"),
            include_str!("checks_current.rs"),
        ),
    ] {
        assert_eq!(arm(old, variant), base);
        assert_eq!(arm(new, variant), arm(production, variant));
        assert_eq!(
            format!("{:x}", Sha256::digest(base)),
            pins["arms"][name]["reference"]
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(arm(new, variant))),
            pins["arms"][name]["current"]
        );
        for helper in ["head", "request", "commit_sha", "decode_stored"] {
            assert_eq!(function(old, helper), function(new, helper));
            assert_eq!(function(new, helper), function(production, helper));
            assert_eq!(
                format!("{:x}", Sha256::digest(function(old, helper))),
                pins["helpers"][helper]
            );
        }
    }
    for (module, copy, source) in [
        (
            "production",
            include_str!("production_helpers.rs"),
            include_str!("../../src/production.rs"),
        ),
        (
            "goals",
            include_str!("goals_helpers.rs"),
            include_str!("../../src/goals.rs"),
        ),
        (
            "memory",
            include_str!("memory_helpers.rs"),
            include_str!("../../src/memory.rs"),
        ),
        (
            "evidence",
            include_str!("evidence_helpers.rs"),
            include_str!("../../src/evidence.rs"),
        ),
        (
            "selected",
            include_str!("selected_helpers.rs"),
            include_str!("../../src/adapters/prismabuild/results.rs"),
        ),
        (
            "clipped",
            include_str!("clipped_helpers.rs"),
            include_str!("../../src/findings.rs"),
        ),
    ] {
        for (key, pin) in pins["closures"].as_object().unwrap() {
            if key.starts_with(&format!("{module}::")) {
                let f = pin["name"].as_str().unwrap();
                assert_eq!(function(copy, f), function(source, f), "{key}");
                assert_eq!(
                    format!("{:x}", Sha256::digest(function(copy, f))),
                    pin["sha256"]
                );
            }
        }
    }
    let schema = include_str!("request_type.rs");
    assert!(production.contains(schema));
    assert_eq!(
        format!("{:x}", Sha256::digest(schema)),
        pins["request_type"]
    );
}
fn schedule(step: Step, reads: Vec<Read>) -> Schedule {
    Schedule {
        steps: vec![step],
        reads,
        ..Schedule::default()
    }
}
fn plan_step() -> Step {
    Step {
        arm: 1,
        ..Step::default()
    }
}
fn checks_step() -> Step {
    Step {
        arm: 2,
        ..Step::default()
    }
}
fn stored_plan() -> Read {
    Ok(Some(json!({"check_run":check_run()})))
}
fn fixes() -> Value {
    json!({"head_sha":SHA,"reviewed_sha":OTHER,"findings":[{"tests":["old"],"reason":"old","problem":"first"},{"problem":"second"},{"tests":null}]})
}
fn fallback(brief: Option<Value>, plan: Option<Value>) -> Vec<Read> {
    vec![Ok(None), Ok(brief), Ok(plan)]
}
fn brief() -> Value {
    json!({"brief":"accepted","goals":[{"id":"goal-1"}]})
}
fn leaf() -> Value {
    json!({"kind":"leaf","files":["src/a.rs"],"checks":[]})
}
fn row<'a>(r: &'a Run, table: &str, id: &str) -> &'a Value {
    &r.persisted_rows[&(table.into(), id.into())]
}
fn event(r: &Run) -> &Value {
    &r.results[0]["event"]["Reply"]["reply"]["ChecksDone"]
}
fn error(r: &Run) -> &str {
    r.results[0]["error"].as_str().unwrap()
}
#[test]
fn mandatory_preparation_skip_routes_never_read_store() {
    for step in [
        Step {
            present: false,
            ..Step::default()
        },
        Step {
            node: 1,
            ..Step::default()
        },
        Step {
            adopted: false,
            ..Step::default()
        },
        Step {
            same_head: false,
            ..Step::default()
        },
        Step {
            snapshot_head: None,
            ..Step::default()
        },
        Step {
            snapshot_head: Some(OTHER.into()),
            ..Step::default()
        },
    ] {
        let r = compare(&schedule(step, vec![Err("must not read")]));
        assert_eq!(
            r.calls
                .iter()
                .map(|c| c["op"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["translation.result", "transaction.commit"]
        );
        assert!(r.staged_rows.is_empty());
    }
}
#[test]
fn mandatory_preparation_stored_run_overrides_only_repository_and_base() {
    let mut run = check_run();
    run.reviewed_sha = Some(tl3_worktrees::CommitId::new(SHA).unwrap());
    run.reproducing_unavailable = Some("wording".into());
    run.selected_failures = vec!["tests/old.py".into()];
    let r = compare(&schedule(
        Step::default(),
        vec![Ok(Some(json!({"check_run":run})))],
    ));
    let saved = row(&r, "check_plans", &format!("0:{SHA}"));
    assert_eq!(saved["repository"], "/new-repository");
    assert_eq!(saved["base_sha"], OTHER);
    assert_eq!(saved["reviewed_sha"], SHA);
    assert_eq!(saved["plan"], json!(run.plan));
    assert_eq!(saved["selected_failures"], json!(run.selected_failures));
    assert_eq!(r.calls[0]["op"], "worker_materials");
    assert_eq!(r.calls[1]["op"], "row.push");
    assert_eq!(r.writes, 1);
}
#[test]
fn mandatory_preparation_fallback_read_order_and_no_check_run() {
    for materials in [None, Some(json!({})), Some(json!({"other":1}))] {
        let reads = vec![Ok(materials), Ok(Some(brief())), Ok(Some(leaf()))];
        let r = compare(&schedule(Step::default(), reads));
        assert_eq!(
            row(&r, "check_plans", &format!("0:{SHA}"))["plan"],
            json!([])
        );
        assert_eq!(
            r.calls
                .iter()
                .take(3)
                .map(|c| c["op"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["worker_materials", "worker_document", "worker_document"]
        );
        assert_eq!(r.calls[1]["args"]["table"], "briefs");
        assert_eq!(r.calls[2]["args"]["table"], "plans");
    }
}
#[test]
fn mandatory_preparation_all_read_parse_and_commit_errors() {
    for reads in [
        vec![Err("materials")],
        vec![Ok(Some(json!({"check_run":null})))],
        vec![Ok(Some(json!({"check_run":{}})))],
        vec![Ok(None), Err("brief")],
        fallback(None, Some(leaf())),
        vec![Ok(None), Ok(Some(brief())), Err("plan")],
        fallback(Some(brief()), None),
        fallback(Some(brief()), Some(json!({"kind":"branch"}))),
        fallback(Some(json!({"goals":null})), Some(leaf())),
        fallback(Some(json!({"goals":[0]})), Some(leaf())),
        fallback(Some(json!({"goals":[{"id":""}]})), Some(leaf())),
        fallback(Some(json!({"goals":[{"id":"x"},{"id":"x"}]})), Some(leaf())),
        fallback(Some(brief()), Some(json!({"kind":"leaf","checks":null}))),
        fallback(Some(brief()), Some(json!({"kind":"leaf","checks":[{}]}))),
        fallback(
            Some(brief()),
            Some(json!({"kind":"leaf","checks":[{"goal_ids":[99]}]})),
        ),
    ] {
        let r = compare(&schedule(Step::default(), reads));
        assert!(!error(&r).is_empty());
        assert!(r.staged_rows.is_empty());
        assert_eq!(r.writes, 0);
    }
    for reads in [vec![stored_plan()], fallback(Some(brief()), Some(leaf()))] {
        let r = compare(&schedule(
            Step {
                base: "invalid".into(),
                ..Step::default()
            },
            reads,
        ));
        assert!(!error(&r).is_empty());
        assert!(r.staged_rows.is_empty());
    }
    let mut s = schedule(Step::default(), vec![stored_plan()]);
    s.failures = vec![true];
    let r = compare(&s);
    assert!(!error(&r).is_empty());
    assert_eq!(r.staged_rows.len(), 1);
    assert!(r.persisted_rows.is_empty());
}
#[test]
fn mandatory_preexisting_nonempty_adoption_plan_decode_failure() {
    // Real maker_inputs adds goal_ids, but Check denies that field on this base.
    // Keep the existing failure; this extraction does not change the schema.
    let plan = json!({"kind":"leaf","files":["a.rs"],"checks":[{"kind":"regression","node_ids":["tests/a.py"],"goal_ids":[0]}]});
    let r = compare(&schedule(
        Step::default(),
        fallback(Some(brief()), Some(plan)),
    ));
    assert!(error(&r).contains("goal_ids"));
    assert!(r.persisted_rows.is_empty());
    assert_eq!(r.writes, 0);
}
#[test]
fn mandatory_check_plan_read_gate_and_matching_heads() {
    for step in [
        Step {
            present: false,
            ..plan_step()
        },
        Step {
            rebasing: false,
            ..plan_step()
        },
    ] {
        let r = compare(&schedule(step, vec![Err("must not read")]));
        assert_eq!(r.calls[0]["op"], "row.push");
        assert_eq!(r.staged_rows.len(), 1);
    }
    for read in [
        Ok(None),
        Ok(Some(json!({"head_sha":OTHER,"findings":[]}))),
        Ok(Some(json!({"findings":[]}))),
    ] {
        let r = compare(&schedule(plan_step(), vec![read]));
        assert_eq!(r.staged_rows.len(), 1);
        assert_eq!(r.calls[0]["op"], "worker_document");
    }
    let r = compare(&schedule(
        Step {
            snapshot_head: None,
            ..plan_step()
        },
        vec![Ok(Some(json!({"findings":[]})))],
    ));
    assert_eq!(r.staged_rows.len(), 2);
    assert_eq!(row(&r, "review_fixes", "0")["head_sha"], SHA);
    let mut step = plan_step();
    step.sha = OTHER.into();
    let r = compare(&schedule(step, vec![Ok(Some(fixes()))]));
    assert_eq!(row(&r, "review_fixes", "0")["head_sha"], OTHER);
    assert_eq!(row(&r, "review_fixes", "0")["reviewed_sha"], OTHER);
    assert_eq!(r.calls[1]["args"]["put"]["table"], "review_fixes");
    assert_eq!(r.calls[2]["args"]["put"]["table"], "check_plans");
}
#[test]
fn mandatory_check_plan_optional_review_and_unavailable_reason() {
    for reviewed in [false, true] {
        for unavailable in [false, true] {
            let mut step = plan_step();
            step.sha = OTHER.into();
            step.run.reviewed_sha = reviewed.then(|| tl3_worktrees::CommitId::new(SHA).unwrap());
            step.run.reproducing_unavailable = unavailable.then(|| "wording".into());
            let r = compare(&schedule(step, vec![Ok(Some(fixes()))]));
            let saved = row(&r, "review_fixes", "0");
            assert_eq!(saved["reviewed_sha"], if reviewed { SHA } else { OTHER });
            assert_eq!(saved["findings"][0].get("tests").is_none(), unavailable);
            assert_eq!(saved["findings"][2].get("tests").is_none(), unavailable);
            assert!(saved["findings"][1].get("reason").is_none());
            if unavailable {
                assert_eq!(saved["findings"][0]["reason"], "wording");
                assert_eq!(saved["findings"][2]["reason"], "wording");
            }
        }
    }
}
#[test]
fn mandatory_check_plan_errors_and_existing_malformed_findings_panic() {
    for request in [None, Some(json!({}))] {
        let r = compare(&schedule(
            Step {
                request,
                ..plan_step()
            },
            vec![stored_plan()],
        ));
        assert!(!error(&r).is_empty());
        assert!(r.calls.is_empty());
    }
    let r = compare(&schedule(plan_step(), vec![Err("fixes")]));
    assert!(!error(&r).is_empty());
    assert_eq!(r.calls.len(), 1);
    for malformed in [json!({}), json!({"findings":null}), json!({"findings":{}})] {
        let mut f = malformed;
        f["head_sha"] = json!(SHA);
        let mut step = plan_step();
        step.run.reproducing_unavailable = Some("wording".into());
        let s = schedule(step, vec![Ok(Some(f))]);
        let old = io::run(&s, false, None);
        assert!(old.results[0]["panic"].is_string());
        let new = io::run(&s, true, None);
        assert!(error(&new).contains("stored review_fixes"));
        assert_eq!(old.calls, new.calls);
        assert!(new.staged_rows.is_empty());
        assert_eq!(old.persisted_rows, new.persisted_rows);
    }
}
#[test]
fn mandatory_checks_current_generation_records_in_order_and_carries_selected() {
    let r = compare(&schedule(checks_step(), vec![]));
    assert_eq!(r.staged_rows.len(), 3);
    assert!(r.staged_rows[0].get("delete").is_some());
    assert_eq!(r.staged_rows[1]["put"]["table"], "check_evidence");
    assert_eq!(r.staged_rows[2]["put"]["table"], "check_reports");
    assert_eq!(
        row(&r, "check_reports", &format!("0:{SHA}"))["selected_failures"],
        json!(["tests/a.py", "tests/old.py"])
    );
    assert_eq!(event(&r)["pass"], true);
    assert_eq!(event(&r)["in_footprint"], false);
    assert_eq!(r.results[0]["feedback"]["outside"], json!(["src/b.rs"]));
    let ops = r
        .calls
        .iter()
        .map(|c| c["op"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        &ops[..6],
        [
            "intern.select",
            "intern.insert",
            "intern.result",
            "intern.select",
            "intern.insert",
            "intern.result"
        ]
    );
    assert_eq!(&ops[6..9], ["row.push", "row.push", "row.push"]);
    assert_eq!(r.persisted_intern.len(), 4);
}
#[test]
fn mandatory_checks_generation_job_and_head_matrix() {
    for job in [None, Some(6), Some(7)] {
        for current in [6, 7] {
            for same_head in [false, true] {
                for pass in [false, true] {
                    let s = schedule(
                        Step {
                            job,
                            current,
                            same_head,
                            pass,
                            ..checks_step()
                        },
                        vec![],
                    );
                    let r = compare(&s);
                    assert_eq!(
                        r.staged_rows.len(),
                        if job == Some(7) && current == 7 && same_head {
                            3
                        } else {
                            0
                        }
                    );
                    assert_eq!(event(&r)["pass"], pass && same_head);
                    assert_eq!(r.persisted_intern.len(), 4); // Paths still intern after stale replies.
                }
            }
        }
    }
}
#[test]
fn mandatory_checks_empty_collections_current_report_and_other_node_rows() {
    let r = compare(&schedule(
        Step {
            changed: vec![],
            failed: vec![],
            results: vec![],
            prior: vec![
                (format!("1:{OTHER}"), json!({})),
                (format!("0:{SHA}"), json!({})),
            ],
            ..checks_step()
        },
        vec![],
    ));
    assert_eq!(r.staged_rows.len(), 2);
    assert_eq!(event(&r)["in_footprint"], true);
    assert!(
        row(&r, "check_reports", &format!("0:{SHA}"))
            .get("selected_failures")
            .is_none()
    );
    assert_eq!(
        row(&r, "check_evidence", "0:00000000000000000007")["proof_missing"],
        true
    );
}
#[test]
fn mandatory_checks_bounds_unicode_duplicates_and_evidence() {
    let path = "é".repeat(4100);
    let changed = (0..103)
        .map(|i| format!("{path}{i}"))
        .chain([path.clone(), path.clone()])
        .collect::<Vec<_>>();
    let failed = (0..22)
        .map(|_| findings::FailedTest {
            test: "é".repeat(4100),
            message: "文".repeat(findings::MAX_MESSAGE_CHARS + 3),
        })
        .collect();
    let mut step = checks_step();
    step.changed = changed;
    step.failed = failed;
    step.footprint = vec![];
    step.results = vec![
        json!({"name":"forced","kind":"demonstrating","verdict":"pass","forcing":{"environment":{"X":"1"},"patches":[],"symptom":"message"},"proof":{"tree":SHA},"tests":[{"test":"t","base":"failed","head":"passed","head_forced":"passed","passed":true}]}),
    ];
    let r = compare(&schedule(step, vec![]));
    let feedback = &r.results[0]["feedback"];
    assert_eq!(feedback["total"], 105);
    assert_eq!(feedback["outside"].as_array().unwrap().len(), 100);
    assert_eq!(
        feedback["outside"][0].as_str().unwrap().chars().count(),
        4096
    );
    assert_eq!(feedback["failed"].as_array().unwrap().len(), 20);
    assert_eq!(
        feedback["failed"][0]["test"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        4096
    );
    assert_eq!(
        feedback["failed"][0]["message"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        findings::MAX_MESSAGE_CHARS
    );
    assert_eq!(
        row(&r, "check_evidence", "0:00000000000000000007")["forced_passed"],
        1
    );
}
#[test]
fn mandatory_checks_errors_before_after_intern_and_staged_rows() {
    for request in [
        None,
        Some(json!({})),
        Some(json!({"repo":{"owner":"","name":"Repo","installation":1},"issue":1,"issue_id":1})),
    ] {
        let r = compare(&schedule(
            Step {
                request,
                ..checks_step()
            },
            vec![],
        ));
        assert!(!error(&r).is_empty());
        assert!(r.persisted_intern.is_empty());
    }
    for step in [
        Step {
            sha: "invalid".into(),
            ..checks_step()
        },
        Step {
            invalid_footprint: true,
            ..checks_step()
        },
    ] {
        let r = compare(&schedule(step, vec![]));
        assert!(!error(&r).is_empty());
        assert_eq!(r.writes, 1);
        assert!(r.persisted_intern.is_empty());
    }
    for step in [
        Step {
            present: false,
            ..checks_step()
        },
        Step {
            node: u64::MAX,
            ..checks_step()
        },
    ] {
        let r = compare(&schedule(step, vec![]));
        assert!(r.results[0]["panic"].is_string() || r.results[0]["error"].is_string());
        assert!(r.persisted_intern.is_empty());
    }
    let r = compare(&schedule(
        Step {
            changed: vec!["a".into(), "".into()],
            ..checks_step()
        },
        vec![],
    ));
    assert!(!error(&r).is_empty());
    assert_eq!(r.staged_rows.len(), 3);
    assert!(r.persisted_intern.is_empty());
    for index in 0..5 {
        let mut s = schedule(checks_step(), vec![]);
        s.failures = vec![false; index];
        s.failures.push(true);
        let r = compare(&s);
        assert!(!error(&r).is_empty());
        assert!(r.persisted_intern.is_empty());
        assert!(r.persisted_rows.is_empty());
    }
    for index in 0..4 {
        let mut s = schedule(checks_step(), vec![]);
        s.select_failures = vec![false; index];
        s.select_failures.push(true);
        let r = compare(&s);
        assert!(!error(&r).is_empty());
        assert!(r.persisted_intern.is_empty());
    }
}
#[test]
fn mandatory_checks_existing_intern_dictionary_and_all_inside() {
    let mut s = schedule(
        Step {
            footprint: vec![41, 42],
            ..checks_step()
        },
        vec![],
    );
    s.interned
        .insert(("Repo".into(), "owner".into(), "repo".into()), 17);
    s.interned
        .insert(("Commit".into(), "0".into(), SHA.into()), 2);
    s.interned
        .insert(("Path".into(), "17".into(), "src/a.rs".into()), 41);
    s.interned
        .insert(("Path".into(), "17".into(), "src/b.rs".into()), 42);
    let r = compare(&s);
    assert_eq!(r.writes, 1);
    assert_eq!(event(&r)["in_footprint"], true);
    assert_eq!(r.persisted_intern, s.interned);
}
fn step_strategy() -> impl Strategy<Value = Step> {
    (
        0u8..3,
        any::<u8>(),
        any::<u8>(),
        prop::collection::vec("[a-z/]{0,12}", 0..6),
        prop::collection::vec(any::<bool>(), 0..4),
    )
        .prop_map(|(arm, flags, evidence, changed, opts)| {
            let mut s = Step {
                arm,
                changed,
                ..Step::default()
            };
            s.present = flags & 1 == 0;
            s.adopted = flags & 2 == 0;
            s.same_head = flags & 4 == 0;
            s.rebasing = flags & 8 == 0;
            if flags & 16 != 0 {
                s.current = 6;
            }
            if flags & 32 != 0 {
                s.job = None;
            }
            s.pass = flags & 64 == 0;
            if flags & 128 != 0 {
                s.sha = "invalid".into();
            }
            if evidence & 1 != 0 {
                s.snapshot_head = None;
            }
            if evidence & 2 != 0 {
                s.prior.clear();
            }
            if evidence & 4 != 0 {
                s.results.clear();
            }
            if evidence & 8 != 0 {
                s.failed.clear();
            }
            if evidence & 16 != 0 {
                s.request = None;
            }
            if evidence & 32 != 0 {
                s.request = Some(json!({}));
            }
            if opts.first() == Some(&true) {
                s.run.reviewed_sha = Some(tl3_worktrees::CommitId::new(OTHER).unwrap());
            }
            if opts.get(1) == Some(&true) {
                s.run.reproducing_unavailable = Some("wording".into());
            }
            s
        })
}
#[test]
fn fixed_seed_varying_reads_errors_sequences_and_every_write_crash() {
    let strategy = (
        prop::collection::vec(step_strategy(), 1..7),
        prop::collection::vec(0u8..8, 0..20),
        prop::collection::vec(any::<bool>(), 0..16),
    );
    let mut runner = TestRunner::new(Config {
        cases: 384,
        rng_seed: RngSeed::Fixed(0x472d_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(writable_regressions(
            include_str!("proptest-regressions.txt"),
            "check_translate_io-472d2026-regressions.txt",
        )))),
        ..Config::default()
    });
    runner
        .run(&strategy, |(steps, tape, failures)| {
            let reads = tape
                .into_iter()
                .map(|kind| match kind {
                    0 => Err("read"),
                    1 => Ok(None),
                    2 => stored_plan(),
                    3 => Ok(Some(brief())),
                    4 => Ok(Some(leaf())),
                    5 => Ok(Some(fixes())),
                    6 => Ok(Some(json!({"head_sha":OTHER,"findings":[]}))),
                    _ => Ok(Some(json!({}))),
                })
                .map(|read| read.map(|value| value.map(|mut value| {
                    if let Some(fields) = value.as_object_mut() {
                        fields.entry("findings").or_insert_with(|| json!([]));
                    }
                    value
                })))
                .collect();
            compare(&Schedule {
                steps,
                reads,
                failures: failures.clone(),
                select_failures: failures,
                ..Schedule::default()
            });
            Ok(())
        })
        .unwrap();
    eprintln!(
        "G2a2: 384 fixed-seed changing-read/error sequences and all write cuts; seed 0x472d2026"
    );
}
#[test]
fn fixed_seed_reachable_check_sequences() {
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x472e_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(writable_regressions(
            include_str!("proptest-regressions.txt"),
            "check_translate_io-472e2026-regressions.txt",
        )))),
        ..Config::default()
    });
    runner
        .run(
            &prop::collection::vec(
                (
                    any::<bool>(),
                    any::<bool>(),
                    prop::collection::vec("[a-z/]{1,20}", 0..7),
                ),
                1..9,
            ),
            |steps| {
                let steps = steps
                    .into_iter()
                    .map(|(same_head, pass, changed)| Step {
                        same_head,
                        pass,
                        changed,
                        ..checks_step()
                    })
                    .collect::<Vec<_>>();
                let count = steps.len();
                let r = compare(&Schedule {
                    steps,
                    reads: vec![],
                    ..Schedule::default()
                });
                prop_assert_eq!(r.results.len(), count);
                Ok(())
            },
        )
        .unwrap();
    eprintln!("G2a2: 128 reachable check sequences; seed 0x472e2026");
}

fn arbitrary_json() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null), any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)), any::<u64>().prop_map(|n| json!(n)),
        any::<f64>().prop_filter("finite JSON number", |n| n.is_finite()).prop_map(|n| json!(n)),
        ".{0,30}".prop_map(Value::String),
    ].prop_recursive(3, 24, 5, |inner| prop_oneof![
        prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
        prop::collection::btree_map(".{0,12}", inner, 0..5).prop_map(|m| Value::Object(m.into_iter().collect())),
    ])
}

/// #838 / Q7: parsing retains extension fields, nulls and numeric forms.
#[test]
fn review_fix_record_round_trip_and_retarget_io_schedules() {
    let strategy = (
        prop::collection::btree_map(".{0,12}", arbitrary_json(), 0..5),
        prop::collection::vec(prop::collection::btree_map(".{0,12}", arbitrary_json(), 0..5), 0..6),
        any::<bool>(), any::<bool>(),
    );
    let mut runner = TestRunner::new(Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(0x838_2026),
        failure_persistence: None,
        ..Config::default()
    });
    runner.run(&strategy, |(extensions, findings, reviewed, unavailable)| {
        let mut record = Value::Object(extensions.into_iter().collect());
        record["head_sha"] = json!(SHA);
        record["findings"] = json!(findings);
        let typed = findings::ReviewFixes::parse(record.clone()).unwrap();
        prop_assert_eq!(serde_json::to_vec(&json!(typed)).unwrap(), serde_json::to_vec(&record).unwrap());
        let mut step = plan_step();
        step.sha = OTHER.into();
        step.run.reviewed_sha = reviewed.then(|| tl3_worktrees::CommitId::new(OTHER).unwrap());
        step.run.reproducing_unavailable = unavailable.then(|| "before tree unavailable 🦀".into());
        // The tape changes at every read. Both arms must consume it identically,
        // with a crash immediately before and after every write and final commit.
        compare(&Schedule {
            steps: vec![step.clone(), step],
            reads: vec![Ok(Some(record.clone())), Ok(Some(json!({"head_sha":OTHER, "findings":[]})))],
            initial_rows: BTreeMap::from([(("review_fixes".into(), "0".into()), record)]),
            ..Schedule::default()
        });
        Ok(())
    }).unwrap();
    eprintln!("#838: 256 byte round-trips and changing-read schedules, all write/crash cuts, seed 0x8382026");
}
