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
    let end = rest[1..]
        .find(&format!("\n{prefix}"))
        .map(|i| start + i + 2)
        .unwrap_or_else(|| source[start..].find("\n        }\n").unwrap() + start + 1);
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
    for (name, old) in [
        ("Prerequisite", include_str!("base-Prerequisite.rs.txt")),
        ("Classified", include_str!("base-Classified.rs.txt")),
        ("Postmortem", include_str!("base-Postmortem.rs.txt")),
        ("LinkedRequest", include_str!("base-LinkedRequest.rs.txt")),
        ("Steering", include_str!("base-Steering.rs.txt")),
    ] {
        assert_eq!(arm(translate, name), old, "delegate-only {name}");
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
                assert_eq!(run.calls[0]["args"]["id"], "owner/repo/3/7");
                assert_eq!(run.calls[1]["args"]["table"], "github_comments");
                assert_eq!(run.calls[2]["args"]["table"], "unclassified_comments");
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
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/translate_misc_io/proptest-regressions.txt"
        )))),
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
