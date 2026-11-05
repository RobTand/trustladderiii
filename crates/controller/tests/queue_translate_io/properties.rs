use super::*;
use io::{NEW_HEAD, OLD_HEAD, Schedule, compare, incident, plan, row, stable};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};

fn required() -> adapters::mergify::RequiredCheck {
    adapters::mergify::RequiredCheck {
        context: "CI".into(),
        app: Some(1),
    }
}
fn evidence(tag: u8, head: &str, flags: u8) -> Evidence {
    match tag % 11 {
        0 => Evidence::ChecksUnavailable {
            permissions: vec![
                "statuses:403".into(),
                "checks:403".into(),
                "statuses:403".into(),
            ],
        },
        1 => Evidence::CheckMissing {
            head: head.into(),
            required: required(),
        },
        2 => Evidence::QueueBlocked {
            head: head.into(),
            conditions: vec!["label=approved".into()],
            failed: vec!["optional CI".into()],
        },
        3 => Evidence::CheckReported {
            head: head.into(),
            required: required(),
        },
        4 => Evidence::Merged {
            head: head.into(),
            checks_passed: flags & 1 != 0,
        },
        5 => Evidence::CiFailed {
            check: 42,
            completed_at: Some("revision".into()),
            head: head.into(),
            infra: flags & 1 != 0,
            head_check: flags & 2 != 0,
            details: findings::QueueCiEvidence {
                checks: vec![],
                failed_tests: vec![findings::FailedTest {
                    test: "tests/outside.py::case".into(),
                    message: "failed".into(),
                }],
                changed_paths: Some(vec!["src/changed.rs".into()]),
                complete: true,
                decision: None,
            },
        },
        6 => Evidence::Conflict {
            check: 42,
            completed_at: None,
            head: head.into(),
        },
        7 => Evidence::DequeuedByCommand {
            check: 42,
            completed_at: None,
            head: head.into(),
        },
        8 => Evidence::DequeuedByPerson { event: 42 },
        9 => Evidence::ClosedByPerson { event: 42 },
        _ => Evidence::PushByOther { head: head.into() },
    }
}
fn event(run: &io::Run) -> &Value {
    &run.result["event"]["Fact"]["fact"]
}
fn has_table(run: &io::Run, table: &str) -> bool {
    run.rows.iter().any(|r| r["table"] == table)
}
fn prior(s: &mut Schedule, mask: u8) {
    let mut value = json!({"repository":s.fact.repository,"number":s.fact.number,
        "evidence":{"CiFailed":{"head":OLD_HEAD,"infra":true,"details":{"failed_tests":[{"test":"tests/outside.py::case"}]}}}});
    if mask & 1 != 0 {
        value["repository"]["name"] = json!("other");
    }
    if mask & 2 != 0 {
        value["number"] = json!(101);
    }
    if mask & 4 != 0 {
        value["evidence"]["CiFailed"]["head"] = json!(NEW_HEAD);
    }
    if mask & 8 != 0 {
        value["evidence"]["CiFailed"]["infra"] = json!(false);
    }
    if mask & 16 != 0 {
        value["evidence"]["CiFailed"]["details"]["failed_tests"] =
            json!([{"test":"tests/different.py::case"}]);
    }
    if mask & 32 != 0 {
        value["evidence"]["CiFailed"]["details"]["failed_tests"] = json!({"not":"an array"});
    }
    if mask & 64 != 0 {
        value["evidence"]["CiFailed"]["details"]["failed_tests"] =
            json!([null,{"test":42},{"test":"tests/outside.py::case"}]);
    }
    s.snapshot.rows.push(row(
        if mask & 128 != 0 {
            "other"
        } else {
            "queue_facts"
        },
        "prior",
        value,
    ));
}
fn add_incidents(s: &mut Schedule, open: bool) {
    let ids = match &s.fact.evidence {
        Evidence::ChecksUnavailable { permissions } => permissions
            .iter()
            .map(|p| looks::check_setup_incident_id(NodeId(0), p))
            .collect(),
        Evidence::CheckMissing { head, required } | Evidence::CheckReported { head, required } => {
            vec![looks::missing_check_incident_id(NodeId(0), head, required)]
        }
        Evidence::QueueBlocked {
            head, conditions, ..
        } => vec![looks::queue_blocked_incident_id(
            NodeId(0),
            head,
            conditions,
        )],
        _ => vec![],
    };
    for id in ids {
        s.snapshot
            .rows
            .push(row("incidents", &id, incident(&id, open, "existing")));
    }
}

/// S-once / #751/#760: the receipt guard runs before node decoding, CI reads
/// and intern writes. Exercise the actual old/new Queue bodies in every cell.
#[test]
fn mandatory_recorded_open_closed_matrix() {
    let mut cells = 0;
    for phase in 0..4 {
        for merged in [false, true] {
            for recorded in [false, true] {
                for tag in [4, 5, 10] {
                    let head = if tag == 5 { OLD_HEAD } else { NEW_HEAD };
                    let mut s = stable(evidence(tag, head, 0));
                    s.phase = phase;
                    s.merged = merged;
                    if recorded {
                        s.snapshot.rows.push(row(
                            "queue_facts",
                            &s.fact.identity(),
                            json!(&s.fact),
                        ));
                    }
                    let run = compare(&s);
                    let kernel = phase != 2 || (tag == 4 && !merged);
                    assert_eq!(!run.result["event"].is_null(), !recorded && kernel);
                    if recorded {
                        assert!(run.rows.is_empty());
                        assert!(run.effects.is_empty());
                        assert_eq!(run.writes, 0);
                        assert_eq!(run.calls, [json!({"op":"transaction.commit","args":null})]);
                        // Each failure would be reached if the guard moved later.
                        s.snapshot.rows.iter_mut().find(|r| r.table == "pull_requests").unwrap().id = "malformed".into();
                        s.plans = vec![Err("recorded facts must not read plans")];
                        s.intern_failures = vec![true];
                        let skipped = compare(&s);
                        assert!(skipped.result["event"].is_null());
                        assert!(skipped.rows.is_empty());
                        assert_eq!(skipped.calls, run.calls);
                    }
                    cells += 1;
                }
            }
        }
    }
    assert_eq!(cells, 48);
    eprintln!("S-once guard: 48 recorded/unrecorded x phase x merged x evidence cells, plus 24 poisoned-binding schedules and crash replay");
}

#[test]
fn mandatory_closed_and_stale_head() {
    for tag in 0..11 {
        let mut s = stable(evidence(tag, NEW_HEAD, 0));
        s.phase = 2;
        let run = compare(&s);
        if tag == 4 {
            assert!(event(&run).get("PrMerged").is_some());
        } else {
            assert!(run.rows.is_empty());
            assert!(run.result["event"].is_null());
        }
    }
    let mut s = stable(evidence(4, NEW_HEAD, 1));
    s.phase = 2;
    s.merged = true;
    assert!(compare(&s).rows.is_empty());
    for tag in [5, 6, 7] {
        let run = compare(&stable(evidence(tag, NEW_HEAD, 0)));
        assert!(run.result["event"].is_null());
        assert!(has_table(&run, "queue_facts"));
        assert_eq!(run.writes, 1);
    }
}
#[test]
fn mandatory_setup_incidents_and_dedup() {
    for tag in [0, 1, 2] {
        let mut s = stable(evidence(tag, OLD_HEAD, 0));
        let run = compare(&s);
        assert!(has_table(&run, "incidents"));
        assert!(has_table(&run, "queue_facts"));
        assert!(run.result["event"].is_null());
        assert_eq!(run.writes, 0);
        assert!(!run.effects.is_empty());
        add_incidents(&mut s, true);
        assert!(compare(&s).rows.is_empty());
        let mut cleared = stable(evidence(tag, OLD_HEAD, 0));
        add_incidents(&mut cleared, false);
        let run = compare(&cleared);
        if tag == 0 {
            assert!(has_table(&run, "incidents"));
        } else {
            assert!(run.rows.is_empty());
        }
    }
    for tag in [1, 2] {
        let s = stable(evidence(tag, NEW_HEAD, 0));
        assert!(compare(&s).rows.is_empty());
    }
    let mut queued = stable(evidence(2, OLD_HEAD, 0));
    queued.phase = 1;
    queued.enqueue = false;
    assert!(has_table(&compare(&queued), "incidents"));
}
#[test]
fn mandatory_check_reported_clears_only_own_open_incident() {
    let mut s = stable(evidence(3, OLD_HEAD, 0));
    add_incidents(&mut s, true);
    s.snapshot.rows.insert(
        0,
        row(
            "incidents",
            "unrelated",
            incident("unrelated", true, "leave open"),
        ),
    );
    s.phase = 3;
    let run = compare(&s);
    assert_eq!(run.rows.len(), 1);
    assert_eq!(run.rows[0]["value"]["open"], false);
    assert_ne!(run.rows[0]["id"], "unrelated");
    assert!(run.result["event"].is_null());
    assert_eq!(run.writes, 0);
    let mut absent = stable(evidence(3, OLD_HEAD, 0));
    assert!(compare(&absent).rows.is_empty());
    add_incidents(&mut absent, false);
    assert!(compare(&absent).rows.is_empty());
}
#[test]
fn mandatory_repeated_outside_and_upstream_ci() {
    let mut s = stable(evidence(5, OLD_HEAD, 0));
    s.plans = vec![Ok(Some(plan("outside")))];
    assert_eq!(event(&compare(&s))["CiFailed"]["infra"], true);
    prior(&mut s, 0);
    let run = compare(&s);
    assert_eq!(event(&run)["CiFailed"]["infra"], false);
    assert_eq!(
        run.rows.last().unwrap()["value"]["evidence"]["CiFailed"]["details"]["decision"],
        "repeated_test"
    );
    for mask in [1, 2, 4, 8, 16, 32, 128] {
        let mut s = stable(evidence(5, OLD_HEAD, 0));
        s.plans = vec![Ok(Some(plan("outside")))];
        prior(&mut s, mask);
        assert_eq!(event(&compare(&s))["CiFailed"]["infra"], true);
    }
    for infra in [false, true] {
        let s = stable(evidence(5, OLD_HEAD, u8::from(infra)));
        let run = compare(&s);
        assert_eq!(event(&run)["CiFailed"]["infra"], infra);
        assert_eq!(
            run.rows.last().unwrap()["value"]["evidence"]["CiFailed"]["details"]["decision"],
            "upstream_classification"
        );
    }
}
#[test]
fn mandatory_push_previous_sha_and_plan_carry_forward() {
    let mut s = stable(evidence(10, NEW_HEAD, 0));
    s.plans = vec![Ok(Some(plan("first"))), Ok(Some(plan("second")))];
    let run = compare(&s);
    assert_eq!(run.calls[0]["op"], "check_run");
    assert_eq!(run.calls[0]["args"]["sha"], OLD_HEAD);
    assert_eq!(run.rows[0]["table"], "check_plans");
    assert_eq!(run.rows[0]["id"], format!("0:{NEW_HEAD}"));
    assert_eq!(run.rows[0]["value"], json!(plan("first")));
    assert!(event(&run).get("PushByOther").is_some());
    assert_eq!(run.writes, 1);
    assert_eq!(
        run.calls.iter().filter(|c| c["op"] == "check_run").count(),
        1
    );
    assert!(
        run.persisted_intern
            .contains_key(&("0".into(), NEW_HEAD.into()))
    );
    s.current = None;
    s.plans = vec![Ok(None), Ok(Some(plan("must not be sampled")))];
    let run = compare(&s);
    assert_eq!(run.calls[0]["args"]["sha"], Value::Null);
    assert!(!has_table(&run, "check_plans"));
}
#[test]
fn mandatory_head_check_admission_and_all_kernel_routes() {
    for phase in 0..4 {
        for enqueue in [false, true] {
            for cancelling in [false, true] {
                let mut s = stable(evidence(5, OLD_HEAD, 2));
                s.phase = phase;
                s.enqueue = enqueue;
                s.cancelling = cancelling;
                let run = compare(&s);
                let admitted = phase == 1 || phase == 0 && enqueue && !cancelling;
                assert_eq!(!run.result["event"].is_null(), admitted);
                assert!(!run.calls.iter().any(|c| c["op"] == "check_run"));
            }
        }
    }
    for tag in 4..11 {
        let run = compare(&stable(evidence(tag, OLD_HEAD, 0)));
        assert!(!run.result["event"].is_null());
        assert!(has_table(&run, "queue_facts"));
    }
}
#[test]
fn queue_io_schedules() {
    let mut runner = TestRunner::new(Config {
        cases: 256,
        max_shrink_iters: 2048,
        rng_seed: RngSeed::Fixed(0x0047_1760_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/queue_translate_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    let strategy = (
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        prop::collection::vec(any::<u8>(), 0..32),
        prop::collection::vec(0u8..5, 0..8),
        prop::collection::vec(any::<bool>(), 0..3),
    );
    let comparisons = std::cell::Cell::new(0);
    runner
        .run(
            &strategy,
            |(tag, flags, shape, history, reads, failures)| {
                let mut s = stable(evidence(
                    tag,
                    if flags & 4 != 0 { NEW_HEAD } else { OLD_HEAD },
                    flags,
                ));
                s.phase = (shape >> 4) % 4;
                s.enqueue = shape & 1 != 0;
                s.cancelling = shape & 2 != 0;
                s.merged = shape & 4 != 0;
                s.current = if shape & 8 != 0 { None } else { Some(1) };
                s.approved = if shape & 64 != 0 { None } else { Some(1) };
                if shape & 128 != 0 {
                    add_incidents(&mut s, flags & 8 != 0);
                }
                for mask in history {
                    prior(&mut s, mask);
                }
                s.plans = reads
                    .into_iter()
                    .enumerate()
                    .map(|(n, r)| match r {
                        0 => Ok(None),
                        1 => Err("changing check_run error"),
                        _ => Ok(Some(plan(&format!("sample-{n}-{r}")))),
                    })
                    .collect();
                s.intern_failures = failures;
                let run = compare(&s);
                comparisons.set(comparisons.get() + 1 + run.writes * 2);
                Ok(())
            },
        )
        .unwrap();
    eprintln!(
        "Queue: 256 schedules, {} complete/crash comparisons, seed 0x004717602026",
        comparisons.get()
    );
}
fn arm(source: &str) -> &str {
    let start = source
        .find("            Observation::Queue(mut fact) => {")
        .unwrap();
    let end = source[start..]
        .find("            Observation::GitHubComment {")
        .map(|at| start + at)
        .unwrap_or_else(|| source[start..].find("\n        };\n").unwrap() + start + 1);
    &source[start..end]
}
#[test]
fn pinned_reference_and_current_source() {
    assert_eq!(
        format!("{:x}", Sha256::digest(include_bytes!("base-queue.rs.txt"))),
        "c616896562116b10a862ab3150a83c54cbd8e3b6a37f0fb9c6e9c042d82e6337"
    );
    assert_eq!(
        arm(include_str!("reference.rs")),
        include_str!("base-queue.rs.txt")
    );
    assert_eq!(
        arm(include_str!("current.rs")),
        arm(include_str!("../../src/lib.rs"))
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(include_bytes!("reference_core.rs"))),
        "e32a5550cf68f4228b28fc931d09a3c07fe81171e368f49bac93eb9ea0576f46"
    );
    assert!(include_str!("reference.rs").starts_with("use crate::reference_core as tl3_controller_core;\n"));
    for (bytes, expected) in [
        (
            include_bytes!("base-adapter.rs.txt").as_slice(),
            "4c23cc4a17eb17793dfe1cfc4b911de49660267bbebb306269e8bc673eb255f9",
        ),
        (
            include_bytes!("base-looks.rs.txt").as_slice(),
            "2ad7e468cb0a8d3d22cfe5511b0aa6193cb8236dbca7f8fd7f10a7784b315bee",
        ),
        (
            include_bytes!("base-ci.rs.txt").as_slice(),
            "6c5536439762f6d7a7db7db03bbee0945a9e2829691e16c1e209519e6f44c016",
        ),
    ] {
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected);
    }
}

/// Q52/#447: the unchanged stale-head helper clears setup incidents outside
/// translate. Exercised at the common boundary because this arm never calls it.
#[test]
fn mandatory_missing_and_blocked_clear_with_head() {
    for tag in [1, 2] {
        let mut s = stable(evidence(tag, OLD_HEAD, 0));
        let run = compare(&s);
        assert!(
            run.rows
                .iter()
                .any(|row| row["table"] == "incidents" && row["value"]["open"] == true)
        );
        for change in run.rows {
            s.snapshot.rows.push(row(
                change["table"].as_str().unwrap(),
                change["id"].as_str().unwrap(),
                change["value"].clone(),
            ));
        }
        io::clear_stale(&s);
    }
}

/// Large-module conformance uses step sequences, with previous rows and intern
/// entries carried into each next snapshot rather than an independent mock reset.
#[test]
fn queue_event_sequences() {
    let mut runner = TestRunner::new(Config {
        cases: 128,
        max_shrink_iters: 1024,
        rng_seed: RngSeed::Fixed(0x0047_1760_2027),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/queue_translate_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(
            &prop::collection::vec((any::<u8>(), any::<u8>(), any::<u8>()), 1..24),
            |frames| {
                let mut s = stable(evidence(4, NEW_HEAD, 0));
                for (tag, flags, phase) in frames {
                    s.fact.evidence =
                        evidence(tag, if flags & 4 != 0 { NEW_HEAD } else { OLD_HEAD }, flags);
                    s.phase = phase % 4;
                    s.plans = vec![
                        Ok(Some(plan("sequence-first"))),
                        Ok(Some(plan("sequence-next"))),
                    ];
                    let run = compare(&s);
                    for change in run.persisted_rows {
                        let table = change["table"].as_str().unwrap();
                        let id = change["id"].as_str().unwrap();
                        if let Some(existing) = s
                            .snapshot
                            .rows
                            .iter_mut()
                            .find(|r| r.table == table && r.id == id)
                        {
                            existing.value = change["value"].clone();
                        } else {
                            s.snapshot
                                .rows
                                .push(row(table, id, change["value"].clone()));
                        }
                    }
                    s.snapshot.interned = run
                        .persisted_intern
                        .into_iter()
                        .map(|((scope, value), id)| tl3_store::Interned {
                            scope,
                            value,
                            id: id as i64,
                            kind: InternKind::Commit,
                        })
                        .collect();
                }
                Ok(())
            },
        )
        .unwrap();
    eprintln!("Queue: 128 persisted event sequences, seed 0x004717602027");
}

fn function<'a>(source: &'a str, name: &str, indent: &str) -> &'a str {
    let start = source
        .find(&format!("{indent}pub(crate) fn {name}("))
        .or_else(|| source.find(&format!("{indent}fn {name}(")))
        .unwrap();
    let end = source[start..].find(&format!("\n{indent}}}\n")).unwrap() + start + indent.len() + 3;
    &source[start..end]
}
/// The shared fake boundary compiles whole unchanged helper bodies, and these
/// comparisons refuse drift in either the base copy or today's production.
#[test]
fn shared_helper_bodies_are_verbatim() {
    for name in ["node", "recorded", "identity"] {
        assert_eq!(
            function(include_str!("base-adapter.rs.txt"), name, "    "),
            function(include_str!("queue-fact.rs"), name, "    ")
        );
        assert_eq!(
            function(include_str!("../../src/adapters/mergify.rs"), name, "    "),
            function(include_str!("queue-fact.rs"), name, "    ")
        );
    }
    for name in [
        "check_setup_incident",
        "check_setup_gap",
        "missing_check_incident",
        "queue_blocked_incident",
        "resolve_stale_missing_checks",
    ] {
        assert_eq!(
            function(include_str!("base-looks.rs.txt"), name, "    "),
            function(include_str!("incident-helpers.rs"), name, "    ")
        );
        assert_eq!(
            function(include_str!("../../src/looks.rs"), name, "    "),
            function(include_str!("incident-helpers.rs"), name, "    ")
        );
    }
    for name in [
        "queue_blocked_incident_id",
        "missing_check_incident_id",
        "check_setup_incident_id",
        "queue_blocked_summary",
    ] {
        assert_eq!(
            function(include_str!("base-looks.rs.txt"), name, ""),
            function(include_str!("incident-helpers.rs"), name, "")
        );
        assert_eq!(
            function(include_str!("../../src/looks.rs"), name, ""),
            function(include_str!("incident-helpers.rs"), name, "")
        );
    }
    for name in [
        "terminal_text",
        "safe",
        "bounded",
        "test_path",
        "outside_change",
    ] {
        assert_eq!(
            function(include_str!("base-ci.rs.txt"), name, ""),
            function(include_str!("ci-evidence.rs"), name, "")
        );
        assert_eq!(
            function(
                include_str!("../../src/adapters/mergify/ci_evidence.rs"),
                name,
                ""
            ),
            function(include_str!("ci-evidence.rs"), name, "")
        );
    }
}

fn shaped_schedule(tag: u8, bits: u16) -> Schedule {
    let mut s = stable(evidence(
        tag,
        if bits & 1 == 0 { OLD_HEAD } else { NEW_HEAD },
        bits as u8,
    ));
    match &mut s.fact.evidence {
        Evidence::ChecksUnavailable { permissions } => {
            if bits & 2 == 0 {
                permissions.clear();
            }
        }
        Evidence::CheckMissing { required, .. } | Evidence::CheckReported { required, .. } => {
            if bits & 2 == 0 {
                required.app = None;
            }
        }
        Evidence::QueueBlocked {
            conditions, failed, ..
        } => {
            if bits & 2 == 0 {
                conditions.clear();
            }
            if bits & 4 == 0 {
                failed.clear();
            }
        }
        Evidence::CiFailed {
            completed_at,
            details,
            ..
        } => {
            if bits & 2 == 0 {
                *completed_at = None;
            }
            if bits & 4 != 0 {
                details.checks.push(findings::CiFailure {
                    name: "nonempty check".into(),
                    summary: "failed check details".into(),
                    url: (bits & 8 != 0).then(|| "https://example.invalid/check/42".into()),
                });
            }
            if bits & 16 == 0 {
                details.failed_tests.clear();
            }
            details.changed_paths = match (bits >> 5) % 3 {
                0 => None,
                1 => Some(vec![]),
                _ => Some(vec!["src/changed.rs".into()]),
            };
            details.complete = bits & 128 != 0;
        }
        Evidence::Conflict { completed_at, .. }
        | Evidence::DequeuedByCommand { completed_at, .. } => {
            *completed_at = (bits & 2 != 0).then(|| "another completion".into());
        }
        _ => {}
    }
    let mut run = plan("all-fields");
    if bits & 256 != 0 {
        run.plan.push(adapters::prismabuild::Check {
            name: "planned regression".into(),
            kind: adapters::prismabuild::Kind::Regression,
            tests: vec![
                "tests/planned.py::case".into(),
                "//crates/example:test".into(),
            ],
            forcing: None,
            proof: None,
            declared_symptom: None,
        });
        run.reviewed_sha = Some(tl3_worktrees::CommitId::new(OLD_HEAD).unwrap());
        run.reproducing_unavailable = Some("retained diagnostic".into());
    }
    s.plans = vec![if bits & 512 == 0 {
        Ok(None)
    } else {
        Ok(Some(run))
    }];
    if bits & 1024 != 0 {
        prior(&mut s, 0);
    }
    if bits & 2048 != 0 {
        s.current = None;
    }
    if bits & 4096 != 0 {
        s.snapshot.interned.clear();
    }
    if bits & 8192 != 0 {
        s.snapshot.interned.insert(
            0,
            tl3_store::Interned {
                id: 9,
                kind: InternKind::Repo,
                scope: "other-kind".into(),
                value: "not a commit".into(),
            },
        );
    }
    if bits & 8192 != 0 && bits & 4096 != 0 {
        s.current = Some(9);
    }
    if bits & 16384 != 0 {
        add_incidents(&mut s, true);
    }
    if bits & 32768 != 0 {
        s.snapshot
            .rows
            .push(row("queue_facts", &s.fact.identity(), json!(&s.fact)));
    }
    s
}

/// Every shape executes the complete old/new Queue bodies, including their real
/// incident-helper calls. Direct pure-function witnesses do not replace this.
#[test]
fn generated_optional_and_collection_shapes() {
    let mut runner = TestRunner::new(Config {
        cases: 256,
        max_shrink_iters: 2048,
        rng_seed: RngSeed::Fixed(0x0047_1760_2028),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/queue_translate_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(&(any::<u8>(), any::<u16>()), |(tag, bits)| {
            compare(&shaped_schedule(tag, bits));
            Ok(())
        })
        .unwrap();
}

#[test]
fn mandatory_shell_shapes_and_error_classes() {
    // Exhaust each named collection/optional dimension through the actual shell.
    for bits in [
        0u16,
        u16::MAX,
        1,
        2,
        4,
        8,
        16,
        32,
        64,
        128,
        256,
        512,
        768,
        1024,
        2048,
        4096,
        8192,
        16384,
        32768,
    ] {
        for tag in 0..11 {
            compare(&shaped_schedule(tag, bits));
        }
    }
    let mut push = shaped_schedule(10, 1 | 256 | 512 | 8192);
    let carried = compare(&push);
    let plan_row = carried
        .rows
        .iter()
        .find(|r| r["table"] == "check_plans")
        .unwrap();
    assert_eq!(
        plan_row["value"],
        json!(push.plans[0].as_ref().unwrap().as_ref().unwrap())
    );
    assert_eq!(plan_row["value"]["plan"].as_array().unwrap().len(), 1);
    assert_eq!(carried.calls[0]["args"]["sha"], OLD_HEAD);
    // Present kernel head but no intern row is distinct from absent kernel head.
    push.snapshot.interned.clear();
    assert!(compare(&push).calls[0]["args"]["sha"].is_null());
    push.current = None;
    assert!(compare(&push).calls[0]["args"]["sha"].is_null());

    for tag in [5, 10] {
        let mut read_error = stable(evidence(tag, NEW_HEAD, 0));
        read_error.plans = vec![Err("injected malformed check plan")];
        let result = compare(&read_error);
        assert!(
            result.result["error"]
                .as_str()
                .unwrap()
                .contains("invalid stored controller data")
        );
        assert_eq!(result.writes, 0);
    }
    let mut bad_binding = stable(evidence(4, NEW_HEAD, 0));
    bad_binding
        .snapshot
        .rows
        .iter_mut()
        .find(|r| r.table == "pull_requests")
        .unwrap()
        .id = "not a node".into();
    assert_eq!(compare(&bad_binding).result["error"], "invalid PR binding");
    let mut missing_request = stable(evidence(4, NEW_HEAD, 0));
    missing_request
        .snapshot
        .rows
        .retain(|r| r.table != "requests");
    assert_eq!(
        compare(&missing_request).result["error"],
        "missing request binding"
    );
    let mut malformed_request = stable(evidence(4, NEW_HEAD, 0));
    malformed_request
        .snapshot
        .rows
        .iter_mut()
        .find(|r| r.table == "requests")
        .unwrap()
        .value = Value::Null;
    assert!(
        compare(&malformed_request).result["error"]
            .as_str()
            .unwrap()
            .contains("invalid stored controller data")
    );
    let mut no_binding = stable(evidence(4, NEW_HEAD, 0));
    no_binding
        .snapshot
        .rows
        .retain(|r| r.table != "pull_requests");
    assert!(compare(&no_binding).result["event"].is_null());

    let mut malformed_incident = stable(evidence(3, OLD_HEAD, 0));
    add_incidents(&mut malformed_incident, true);
    malformed_incident
        .snapshot
        .rows
        .iter_mut()
        .find(|r| r.table == "incidents")
        .unwrap()
        .value["summary"] = json!(42);
    assert!(
        compare(&malformed_incident).result["error"]
            .as_str()
            .unwrap()
            .contains("invalid stored controller data")
    );
    let mut staged_plan_failure = shaped_schedule(10, 1 | 256 | 512);
    staged_plan_failure.intern_failures = vec![true];
    let failed = compare(&staged_plan_failure);
    assert!(failed.rows.iter().any(|r| r["table"] == "check_plans"));
    assert!(failed.persisted_rows.is_empty());
    assert_eq!(failed.result["error"], "injected intern failure");
    for tag in [4, 5, 6, 7, 10] {
        let invalid_head = stable(evidence(tag, "invalid-sha", 0));
        let run = compare(&invalid_head);
        assert!(run.result["error"].is_string());
        assert_eq!(run.writes, 0);
        let mut store_error = stable(evidence(tag, NEW_HEAD, 0));
        store_error.intern_failures = vec![true];
        let run = compare(&store_error);
        assert_eq!(run.result["error"], "injected intern failure");
        assert_eq!(run.writes, 1);
        assert!(run.persisted_rows.is_empty());
    }
}

#[test]
fn mandatory_setup_evidence_cannot_reach_kernel_in_shell_or_frozen_route() {
    use tl3_mergify::decide::translate::{self as frozen, Kind, NodeView, Phase, Route};
    let kinds = [
        Kind::ChecksUnavailable,
        Kind::CheckMissing,
        Kind::QueueBlocked,
        Kind::CheckReported,
    ];
    for kind in kinds {
        for phase in [
            Phase::Publishing,
            Phase::Queued,
            Phase::Closed,
            Phase::Other,
        ] {
            for bits in 0u8..16 {
                let view = NodeView {
                    phase,
                    enqueue_pending: bits & 1 != 0,
                    merged: bits & 2 != 0,
                };
                let route = frozen::route(kind, view, bits & 4 != 0, bits & 8 != 0);
                assert!(
                    !matches!(route, Route::Kernel(_)),
                    "setup pair {kind:?}, {view:?}"
                );
            }
        }
    }
    // These are executions of the verbatim old and current Queue shells,
    // including admission, frozen routing, actual incident helpers and sink.
    for tag in 0..4 {
        for phase in 0..4 {
            for bits in 0u8..64 {
                let mut s = stable(evidence(
                    tag,
                    if bits & 1 != 0 { OLD_HEAD } else { NEW_HEAD },
                    0,
                ));
                s.phase = phase;
                s.enqueue = bits & 2 != 0;
                s.cancelling = bits & 4 != 0;
                s.current = if bits & 8 != 0 { Some(1) } else { None };
                s.approved = if bits & 16 != 0 { Some(1) } else { None };
                if bits & 32 != 0 {
                    add_incidents(&mut s, true);
                }
                let run = compare(&s);
                assert!(run.result["event"].is_null(), "setup kernel fact: {s:?}");
                assert!(run.persisted_intern.values().all(|id| *id == 1));
            }
        }
    }
    eprintln!(
        "G1 r1: 256 frozen setup routes and 1024 real-shell setup schedules, plus crash replay"
    );
}

#[test]
fn mandatory_dispatch_rejects_impossible_setup_kernel_pairs() {
    use tl3_controller_core::translate::{Decision, EvidenceKind, KernelKind, Route, begin, dispatch};
    for evidence in [
        EvidenceKind::Setup,
        EvidenceKind::Missing,
        EvidenceKind::Blocked,
        EvidenceKind::Reported,
    ] {
        for kernel in [
            KernelKind::CiFailed { infra: false },
            KernelKind::PrMerged {
                checks_passed: true,
            },
            KernelKind::Conflict,
            KernelKind::PrDequeuedByPerson,
            KernelKind::PrClosedByPerson,
            KernelKind::PushByOther,
        ] {
            assert_eq!(
                dispatch(begin(false).unwrap(), Route::Kernel(kernel), evidence),
                Decision::InvalidKernel
            );
        }
    }
}
