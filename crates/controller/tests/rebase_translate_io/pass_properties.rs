use super::*;
use io::{OTHER, SHA, Schedule, Step, compare};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};
fn schedule(kind: u8) -> Schedule {
    Schedule {
        steps: vec![Step {
            passthrough: Some(kind),
            ..Step::default()
        }],
        ..Schedule::default()
    }
}
fn arm<'a>(s: &'a str, name: &str) -> &'a str {
    let s = s.find("        let event = match observation {").map(|i| &s[i..]).unwrap_or(s);
    let a = s
        .find(&format!(
            "            Observation::{name}{}",
            if name == "Kernel" { "(" } else { " {" }
        ))
        .unwrap();
    let end = s[a + 1..]
        .find("\n            Observation::")
        .map(|o| a + o + 2)
        .unwrap_or_else(|| a + s[a..].find("            _ => unreachable!").unwrap());
    &s[a..end]
}
fn scratch(s: &str) -> &str {
    let s = s.find("        let event = match observation {").map(|i| &s[i..]).unwrap_or(s);
    let a = s.find("            Observation::ScratchSaved {").unwrap();
    let end = s[a..]
        .find("            Observation::Progress {")
        .or_else(|| s[a..].find("            Observation::Kernel("))
        .unwrap()
        + a;
    &s[a..end]
}
fn method<'a>(s: &'a str, name: &str) -> &'a str {
    let a = s.find(&format!("    fn {name}(")).unwrap();
    let b = a + s[a..].find("\n    }\n").unwrap() + 7;
    &s[a..b]
}
#[test]
fn pinned_nine_real_old_current_pass_through_arms_and_helpers() {
    let pins: Value = serde_json::from_str(include_str!("pass-pins.json")).unwrap();
    assert_eq!(pins["base"], "c3e6fbbb3625490d324c487eb1469f6d4794dba2");
    let old = include_str!("pass_reference.rs");
    let new = include_str!("pass_current.rs");
    let production = include_str!("../../src/lib.rs");
    for (name, base) in [
        ("scratch", include_str!("pass-scratch-base.rs.txt")),
        ("Kernel", include_str!("pass-Kernel-base.rs.txt")),
        ("CiFailed", include_str!("pass-CiFailed-base.rs.txt")),
        ("Made", include_str!("pass-Made-base.rs.txt")),
        ("Reviewed", include_str!("pass-Reviewed-base.rs.txt")),
        ("Merged", include_str!("pass-Merged-base.rs.txt")),
        ("Plan", include_str!("pass-Plan-base.rs.txt")),
    ] {
        let extract = |s| {
            if name == "scratch" {
                scratch(s)
            } else {
                arm(s, name)
            }
        };
        assert_eq!(extract(old), base);
        assert_eq!(extract(new), extract(production));
        assert_eq!(extract(old), extract(new));
        assert_eq!(format!("{:x}", Sha256::digest(base)), pins["arms"][name]);
    }
    for name in ["head", "footprint"] {
        assert_eq!(method(old, name), method(production, name));
        assert_eq!(method(new, name), method(production, name));
        assert_eq!(
            format!("{:x}", Sha256::digest(method(old, name))),
            pins["helpers"][name]
        );
    }
}
#[test]
fn mandatory_scratch_saved_shell() {
    let r = compare(&schedule(0));
    assert!(r.results[0]["event"].is_null());
    assert_eq!(r.writes, 1);
    assert!(r.staged_rows.is_empty());
}
#[test]
fn mandatory_scratch_pruned_shell() {
    let r = compare(&schedule(1));
    assert!(r.results[0]["event"].is_null());
    assert_eq!(r.writes, 1);
}
#[test]
fn mandatory_trace_logs_capped_shell() {
    let r = compare(&schedule(2));
    assert!(r.results[0]["event"].is_null());
    assert_eq!(r.writes, 1);
}
#[test]
fn mandatory_raw_kernel_shell() {
    let r = compare(&schedule(3));
    assert_eq!(r.results[0]["event"]["Fact"]["fact"], "Conflict");
    assert_eq!(r.writes, 1);
    let mut s = schedule(3);
    s.steps[0].conflict = false;
    let r = compare(&s);
    assert!(
        r.results[0]["error"]
            .as_str()
            .unwrap()
            .contains("steering constructors")
    );
    assert_eq!(r.writes, 0);
}
#[test]
fn mandatory_ci_failed_shell() {
    for infra in [true, false] {
        let mut s = schedule(4);
        s.steps[0].conflict = infra;
        let r = compare(&s);
        assert_eq!(
            r.results[0]["event"]["Fact"]["fact"]["CiFailed"]["infra"],
            infra
        );
        assert_eq!(r.writes, 1);
    }
}
fn head_arm(kind: u8, reply: &str) {
    let r = compare(&schedule(kind));
    assert_eq!(r.results[0]["event"]["Reply"]["reply"][reply]["head"], 1);
    assert_eq!(r.persisted_intern[&("0".into(), SHA.into())], 1);
    assert_eq!(r.writes, 2);
}
#[test]
fn mandatory_made_shell() {
    head_arm(5, "MakerDone");
}
#[test]
fn mandatory_reviewed_shell() {
    head_arm(6, "Approved");
}
#[test]
fn mandatory_merged_shell() {
    for passed in [false, true] {
        let mut s = schedule(7);
        s.steps[0].conflict = passed;
        let r = compare(&s);
        assert_eq!(r.results[0]["event"]["Fact"]["fact"]["PrMerged"]["head"], 1);
        assert_eq!(
            r.results[0]["event"]["Fact"]["fact"]["PrMerged"]["checks_passed"],
            passed
        );
        assert_eq!(r.writes, 2);
    }
}
#[test]
fn mandatory_plan_shell_interns_repo_then_paths_and_footprint() {
    for paths in [
        vec![],
        vec!["a.rs".into()],
        vec!["z.rs".into(), "a.rs".into(), "z.rs".into()],
    ] {
        for fits in [false, true] {
            let mut s = schedule(8);
            s.steps[0].files = paths.clone();
            s.steps[0].conflict = fits;
            let r = compare(&s);
            assert_eq!(
                r.results[0]["event"]["Reply"]["reply"]["PlanLeaf"]["fits"],
                fits
            );
            let footprint = r
                .calls
                .iter()
                .find(|c| c["op"] == "world.footprint")
                .unwrap();
            assert_eq!(
                footprint["args"].as_array().unwrap().len(),
                paths
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            );
            assert_eq!(
                r.writes,
                paths
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    + 2
            );
        }
    }
}
#[test]
fn mandatory_passthrough_errors_and_existing_dictionary() {
    for kind in 5..=7 {
        for sha in ["invalid".into(), "A".repeat(40), "".into()] {
            let mut s = schedule(kind);
            s.steps[0].sha = sha;
            let r = compare(&s);
            assert!(r.results[0]["error"].is_string());
            assert_eq!(r.writes, 0);
        }
        let mut s = schedule(kind);
        s.interned.insert(("0".into(), SHA.into()), 41);
        let r = compare(&s);
        assert_eq!(r.writes, 1);
        assert_eq!(r.persisted_intern, s.interned);
    }
    for request in [
        None,
        Some(json!({})),
        Some(json!({"repo":{"owner":"","name":"Repo","installation":1},"issue":1,"issue_id":1})),
    ] {
        let mut s = schedule(8);
        s.steps[0].request = request;
        let r = compare(&s);
        assert!(r.results[0]["error"].is_string());
        assert_eq!(r.writes, 0);
    }
    let mut s = schedule(8);
    s.steps[0].files = vec!["a.rs".into(), "".into()];
    let r = compare(&s);
    assert_eq!(r.writes, 2);
    assert!(
        r.results[0]["error"]
            .as_str()
            .unwrap()
            .contains("path must be nonempty")
    );
    assert!(r.persisted_intern.is_empty());
    for kind in 5..=8 {
        for index in 0..if kind == 8 { 4 } else { 2 } {
            let mut s = schedule(kind);
            s.failures = vec![false; index];
            s.failures.push(true);
            let r = compare(&s);
            assert!(r.results[0]["error"].is_string());
            assert!(r.persisted_intern.is_empty());
        }
    }
}
#[test]
fn fixed_seed_passthrough_sequences_and_crash_schedules() {
    let strategy = prop::collection::vec(
        (
            0u8..9,
            any::<bool>(),
            prop::collection::vec("[a-z/]{1,12}", 0..5),
        ),
        1..9,
    );
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x472c_2026),
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
                    .map(|(kind, flag, files)| Step {
                        passthrough: Some(kind),
                        conflict: flag,
                        files,
                        sha: if flag { SHA.into() } else { OTHER.into() },
                        ..Step::default()
                    })
                    .collect(),
                ..Schedule::default()
            };
            let r = compare(&s);
            prop_assert_eq!(r.results.len(), s.steps.len());
            Ok(())
        })
        .unwrap();
    eprintln!("G2a r1: 128 fixed-seed nine-arm sequences and every write cut; seed 0x472c2026");
}
