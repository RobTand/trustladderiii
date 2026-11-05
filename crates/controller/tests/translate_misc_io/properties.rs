use super::*;
use io::{compare, observation, repo, row, stable};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};
use sha2::{Digest, Sha256};
fn arm<'a>(source: &'a str, name: &str) -> &'a str {
    let prefix = "            Observation::";
    let start = source.find(&format!("{prefix}{name}")).unwrap();
    let rest = &source[start..];
    let end = [
        "\n            Observation::",
        "\n            _ =>",
        "\n        }\n",
    ]
    .iter()
    .filter_map(|delimiter| rest[1..].find(delimiter).map(|i| start + i + 2))
    .min()
    .unwrap();
    &source[start..end]
}
#[test]
fn reference_and_current_are_real_arms() {
    let base = include_str!("base-comment.rs.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(base)),
        "fb787c25f653d6312d15462d83b8099e3e72f61e8925c5d1e6d2bcded2729fa4"
    );
    assert_eq!(arm(include_str!("reference.rs"), "GitHubComment"), base);
    let production = include_str!("../../src/lib.rs");
    let translate = &production[production.find("    fn translate(").unwrap()..];
    assert_eq!(
        arm(include_str!("current.rs"), "GitHubComment"),
        arm(translate, "GitHubComment")
    );
    for (name, old, pin) in [
        (
            "Prerequisite",
            include_str!("base-Prerequisite.rs.txt"),
            "9bac7617bca51393ff732d262749b5ac819d323f32a56e2160ee757ab4715a07",
        ),
        (
            "Classified",
            include_str!("base-Classified.rs.txt"),
            "dac52d7b4e22c111f2c8d27a32b95060a6af447cb356c91da1cc84be34da99d4",
        ),
        (
            "Postmortem",
            include_str!("base-Postmortem.rs.txt"),
            "f2ef468249157b7936e27b81c51278aa9c71e5259b70d4c62f502b64de916c33",
        ),
        (
            "LinkedRequest",
            include_str!("base-LinkedRequest.rs.txt"),
            "d566e78ed116a2294835aca8746389defa4b7b2e48e72377f9ac92dc0d22f363",
        ),
        (
            "Steering",
            include_str!("base-Steering.rs.txt"),
            "fb8dc7c4c9d4c7351bb457a1f7ec467622e29a2ff1713a16bfba40f2ec45c446",
        ),
    ] {
        assert_eq!(format!("{:x}", Sha256::digest(old)), pin);
        assert_eq!(arm(translate, name), old, "delegate-only {name}");
        assert_eq!(arm(include_str!("reference.rs"), name), old);
        assert_eq!(arm(include_str!("current.rs"), name), old);
    }
}
#[test]
fn mandatory_author_repository_and_identity() {
    for author in [0, 41, 42, u64::MAX] {
        for mismatch in 0..5 {
            let mut s = stable();
            let mut r = repo();
            match mismatch {
                1 => r.owner = "other".into(),
                2 => r.name = "other".into(),
                3 => r.installation = 4,
                4 => {
                    r.owner = "OWNER".into();
                    r.name = "rEpO".into();
                }
                _ => {}
            }
            s.observations = vec![observation(author, r, 7, String::new())];
            let (run, _) = compare(&s);
            let admitted = author == 42 && (mismatch == 0 || mismatch == 4);
            assert_eq!(run.reads, usize::from(admitted));
            assert_eq!(run.bytes.len(), if admitted { 2 } else { 0 });
            if admitted {
                assert_eq!(run.calls[1]["args"]["id"], "owner/repo/3/7");
                assert_eq!(run.calls[2]["args"]["table"], "github_comments");
                assert_eq!(run.calls[3]["args"]["table"], "unclassified_comments");
            }
        }
    }
    let mut s = stable();
    s.repos.clear();
    assert_eq!(compare(&s).0.reads, 0);
}
#[test]
fn mandatory_retirement_duplicates_and_query_errors() {
    for reply in 0..4 {
        for duplicate in [false, true] {
            let mut s = stable();
            s.reads = vec![reply];
            s.rows = vec![
                row("other", "owner/repo/3/7", Value::Null),
                row("github_comments", "unrelated", json!({"x":[1,2]})),
            ];
            if duplicate {
                s.rows
                    .push(row("github_comments", "owner/repo/3/7", Value::Null));
            }
            let (run, _) = compare(&s);
            assert_eq!(run.reads, 1);
            assert_eq!(
                run.bytes.len(),
                if reply == 0 && !duplicate { 2 } else { 0 }
            );
            assert_eq!(run.results[0].get("error").is_some(), reply >= 2);
        }
    }
}
#[test]
fn mandatory_replays_changing_reads_and_atomic_failure() {
    let mut s = stable();
    s.observations = vec![s.observations[0].clone(); 5];
    s.reads = vec![1, 2, 0, 3, 0];
    let (run, _) = compare(&s);
    assert_eq!(run.reads, 5);
    assert_eq!(run.bytes.len(), 2);
    assert_eq!(
        run.calls.iter().filter(|r| r["op"] == "rows.push").count(),
        2
    );
    let mut s = stable();
    s.observations.push(s.observations[0].clone());
    s.commit_failures = vec![true, false];
    let (run, _) = compare(&s);
    assert_eq!(run.bytes.len(), 2);
    assert_eq!(
        run.calls.iter().filter(|r| r["op"] == "rows.push").count(),
        4
    );
}
#[test]
fn recorded_schedules_and_crashes() {
    let mut runner = TestRunner::new(Config {
        cases: 256,
        max_shrink_iters: 1024,
        rng_seed: RngSeed::Fixed(0x730_012c_2026),
        failure_persistence: Some(Box::new(writable_persistence("proptest-regressions.txt"))),
        ..Config::default()
    });
    let count = std::cell::Cell::new(0usize);
    runner
        .run(
            &(
                prop::collection::vec((0u8..8, 0u8..4, any::<bool>(), 0u8..6, ".{0,48}"), 1..18),
                any::<bool>(),
                any::<bool>(),
            ),
            |(frames, existing, empty)| {
                let s = comment_schedule(frames, existing, empty);
                count.set(count.get() + compare(&s).1);
                Ok(())
            },
        )
        .unwrap();
    eprintln!(
        "G2c: 256 schedules; {} complete/crash comparisons",
        count.get()
    );
}

#[test]
fn real_retirement_query_is_unchanged() {
    let body = include_str!("store-read.rs");
    assert_eq!(
        format!("{:x}", Sha256::digest(body)),
        "21488e3bf7c181e0e662c85ec23af26c1c930ce5de23d68bababc997e806bf20"
    );
    assert!(include_str!("../../../store/src/lib.rs").contains(body));
    assert!(include_str!("store.rs").contains(body.trim_end()));
    assert!(body.contains("SELECT EXISTS (SELECT 1 FROM tl3_events WHERE event_key = $1)"));
}

fn prologue(source: &str) -> &str {
    let translate = &source[source.find("    fn translate(").unwrap()..];
    let start = translate
        .find("        if let Observation::GitHubComment")
        .unwrap();
    let end = translate[start..]
        .find("        let mut ci_details")
        .or_else(|| translate[start..].find("        match observation {"))
        .unwrap()
        + start;
    &translate[start..end]
}
#[test]
fn source_checked_operations_preemption() {
    let base = include_str!("base-prologue.rs.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(base)),
        "6941fd89c7ff53fe7b9d5b89c7b181cb44e34a198d74a03ff21d0d2a874a73fb"
    );
    for source in [
        include_str!("reference.rs"),
        include_str!("current.rs"),
        include_str!("../../src/lib.rs"),
    ] {
        assert_eq!(prologue(source), base);
    }
}
fn operations() -> tl3_config::OperationsIssue {
    tl3_config::OperationsIssue {
        owner: "Owner".into(),
        name: "Repo".into(),
        installation_id: 3,
        number: 73,
    }
}
#[test]
fn mandatory_operations_preemption_and_errors() {
    for configured in [false, true] {
        for reply in 0..4 {
            let mut s = stable();
            s.operations = configured.then(operations);
            s.operations_replies = vec![reply];
            let (run, _) = compare(&s);
            assert_eq!(run.operations_reads, 1);
            assert_eq!(run.reads, usize::from(reply == 0));
            assert_eq!(run.delegate_reads, 0);
            assert_eq!(run.calls[0]["op"], "operations_comment");
            assert_eq!(
                run.calls[0]["args"]["args"]["configuration"].is_null(),
                !configured
            );
            assert_eq!(
                run.bytes.len(),
                match reply {
                    0 => 2,
                    3 => 1,
                    _ => 0,
                }
            );
            assert_eq!(run.results[0].get("error").is_some(), reply == 2);
            if reply != 0 {
                assert!(!run.calls.iter().any(|c| c["op"] == "store.comment_retired"));
            }
        }
    }
    let mut s = stable();
    s.operations = Some(operations());
    s.observations = vec![s.observations[0].clone(); 4];
    s.operations_replies = vec![3, 2, 0, 1];
    assert_eq!(compare(&s).0.operations_reads, 4);
    // Non-preempted comment from a different issue on the configured operations repo.
    let mut s = stable();
    s.operations = Some(operations());
    if let Observation::GitHubComment { issue, .. } = &mut s.observations[0] {
        *issue = 74;
    }
    assert_eq!(compare(&s).0.reads, 1);
}
fn delegated_shapes() -> Vec<(&'static str, Observation)> {
    let mut shapes = Vec::new();
    for kind in [
        tl3_kernel::PrerequisiteKind::Issue,
        tl3_kernel::PrerequisiteKind::PullRequest,
    ] {
        for status in [
            tl3_kernel::DependencyStatus::Pending,
            tl3_kernel::DependencyStatus::Delivered,
            tl3_kernel::DependencyStatus::Abandoned,
        ] {
            shapes.push((
                "external_dependency_fact",
                Observation::Prerequisite(ExternalFact {
                    repository: repo(),
                    kind,
                    number: 19,
                    status,
                }),
            ));
        }
    }
    for reply in [
        None,
        Some(Value::Null),
        Some(json!({"commands":[{"kind":"pause"}]})),
    ] {
        shapes.push((
            "classified",
            Observation::Classified {
                key: "classify-key".into(),
                reply: reply.clone(),
            },
        ));
        for failure in [None, Some("refusal".to_string())] {
            for judge_failure in [
                None,
                Some(tl3_store::JudgeFailure {
                    class: tl3_store::JudgeFailureClass::Http,
                    http_status: Some(429u16.try_into().unwrap()),
                }),
            ] {
                shapes.push((
                    "postmortem_finished",
                    Observation::Postmortem {
                        key: "postmortem-key".into(),
                        reply: reply.clone(),
                        failure: failure.clone(),
                        judge_failure,
                    },
                ));
            }
        }
    }
    for present in [false, true] {
        let issue:tl3_github::Issue=serde_json::from_value(json!({"number":91,"title":"Linked request","body":present.then_some("request text"),"state":"open","state_reason":present.then_some("completed"),"labels":if present {json!([{"name":"request"}])} else {json!([])},"pull_request":present.then(||json!({"url":"https://github.com/Owner/Repo/pull/91"})),"updated_at":"2026-11-05T00:00:00Z"})).unwrap();
        shapes.push((
            "linked_request",
            Observation::LinkedRequest {
                repository: repo(),
                issue,
                original: NodeId(7),
            },
        ));
    }
    for command in [
        Command::Stop,
        Command::Pause,
        Command::Resume,
        Command::ChangeRequest,
    ] {
        shapes.push((
            "command_event",
            Observation::Steering {
                node: NodeId(7),
                command,
            },
        ));
    }
    shapes
}
#[test]
fn mandatory_each_assigned_shape_and_delegate_outcome() {
    let mut comparisons = 0;
    for (name, observation) in delegated_shapes() {
        for reply in 0..6 {
            let mut s = stable();
            s.operations = Some(operations());
            s.rows = vec![row("unrelated", "1", json!({"nonempty":[1]}))];
            s.observations = vec![observation.clone()];
            s.delegate_replies = vec![reply];
            let (run, count) = compare(&s);
            comparisons += count;
            assert_eq!(run.operations_reads, 0);
            assert_eq!(run.reads, 0);
            assert_eq!(run.delegate_reads, 1);
            assert_eq!(run.calls[0]["op"], name);
            assert_eq!(run.calls[0]["args"]["args"]["rows"], 1);
            assert_eq!(
                run.results[0].get("error").is_some(),
                reply == 2 || reply == 4
            );
            if reply != 2 && reply != 4 {
                let returns_event =
                    matches!(name, "classified" | "linked_request" | "command_event")
                        && (reply == 1 || reply == 3);
                assert_eq!(!run.results[0]["event"].is_null(), returns_event);
                assert_eq!(run.bytes.len(), usize::from(reply >= 3));
            }
        }
    }
    let mut s = stable();
    s.observations = delegated_shapes()
        .into_iter()
        .take(6)
        .map(|(_, o)| o)
        .collect();
    s.delegate_replies = vec![0, 3, 4, 2, 1, 5];
    comparisons += compare(&s).1;
    eprintln!("G2c r1: {comparisons} mandatory delegate complete/crash comparisons");
}
// Bazel runfiles are read-only. Copy committed seeds into publishable test output.
fn writable_persistence(name: &str) -> FileFailurePersistence {
    let directory = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("TEST_TMPDIR"))
        .expect("run this test through Bazel with writable test output");
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(name);
    std::fs::write(&path, include_str!("proptest-regressions.txt")).unwrap();
    FileFailurePersistence::Direct(Box::leak(
        path.into_os_string()
            .into_string()
            .unwrap()
            .into_boxed_str(),
    ))
}
#[test]
fn regression_failures_persist_to_writable_output_and_reload() {
    use proptest::test_runner::FailurePersistence;
    let persistence = writable_persistence("persistence-witness.txt");
    let mut runner = TestRunner::new(Config {
        cases: 1,
        rng_seed: RngSeed::Fixed(0x730_012c_2026),
        failure_persistence: Some(Box::new(persistence)),
        ..Config::default()
    });
    assert!(
        runner
            .run(&Just(0u8), |_| Err(TestCaseError::fail(
                "persistence witness"
            )))
            .is_err()
    );
    assert_eq!(persistence.load_persisted_failures2(None).len(), 1);
    let mut replay = TestRunner::new(Config {
        cases: 1,
        failure_persistence: Some(Box::new(persistence)),
        ..Config::default()
    });
    let calls = std::cell::Cell::new(0);
    replay
        .run(&Just(0u8), |_| {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(calls.get(), 2, "one saved failure plus one fresh case");
}
#[test]
fn replay_minimized_original_red_schedule() {
    let input: Value = serde_json::from_str(include_str!("regression-schedules.json")).unwrap();
    assert_eq!(input["minimized_frames"], json!([[5, 0, false, 0, ""]]));
    let frames: Vec<Frame> = serde_json::from_value(input["minimized_frames"].clone()).unwrap();
    let s = comment_schedule(
        frames,
        input["existing"].as_bool().unwrap(),
        input["empty_repositories"].as_bool().unwrap(),
    );
    let (run, count) = compare(&s);
    assert_eq!(run.reads, 1);
    assert_eq!(run.bytes.len(), 2);
    assert_eq!(count, 7);
    eprintln!("G2c r1: replayed original red minimum with {count} complete/crash comparisons");
}

type Frame = (u8, u8, bool, u8, String);
fn comment_schedule(frames: Vec<Frame>, existing: bool, empty: bool) -> io::Schedule {
    let mut s = stable();
    if empty {
        s.repos.clear();
    } else {
        s.repos.push(Repository {
            owner: "unrelated".into(),
            name: "other".into(),
            installation: 5,
        });
    }
    if existing {
        s.rows.push(row(
            "github_comments",
            "owner/repo/3/7",
            json!({"old":true}),
        ));
    }
    s.observations.clear();
    s.reads.clear();
    s.commit_failures.clear();
    for (kind, reply, failed, id, body) in frames {
        let mut r = repo();
        if kind == 2 {
            r.owner = "other".into();
        }
        if kind == 3 {
            r.name = "other".into();
        }
        if kind == 4 {
            r.installation = 4;
        }
        if kind == 5 {
            r.owner = "OWNER".into();
        }
        let author = if kind == 1 { 41 } else { 42 };
        s.observations
            .push(observation(author, r, u64::from(id) + 7, body));
        s.reads.push(reply);
        s.commit_failures.push(failed);
    }
    s
}
