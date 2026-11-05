use super::*;
use io::{Read, Schedule, compare};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};

fn schedule(kind: u8) -> Schedule {
    let mut n = tl3_kernel::Node::new(tl3_kernel::RepoId(0));
    n.phase = match kind {
        0 => Phase::Checking,
        1 => Phase::Reviewing,
        2 => Phase::Making,
        _ => Phase::Queued,
    };
    n.generation = 9;
    n.run = Some(9);
    let event = if kind == 3 {
        Event::Fact {
            node: NodeId(0),
            fact: Fact::CiFailed { infra: false },
        }
    } else {
        Event::Reply {
            node: NodeId(0),
            generation: 9,
            reply: match kind {
                0 => Reply::ChecksDone {
                    report: true,
                    pass: false,
                    in_footprint: true,
                },
                1 => Reply::ChangesRequested,
                2 => Reply::MakerCantMeet,
                _ => Reply::MakerDone {
                    head: tl3_kernel::HeadId(1),
                },
            },
        }
    };
    Schedule {
        event,
        nodes: vec![n],
        detailed: false,
        rows: vec![],
        materials: vec![Read::Value(None)],
        documents: vec![Read::Value(None)],
        failed: vec![],
        outside: vec![],
        total: 0,
        checks: vec![],
    }
}
fn finding(run: &io::Run) -> Value {
    let rows: Value = serde_json::from_slice(&run.buffered).unwrap();
    rows.as_array().unwrap().last().unwrap()["Put"]["value"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone()
}
fn operations(run: &io::Run) -> Vec<&str> {
    run.trace.iter().map(|v| v[0].as_str().unwrap()).collect()
}
#[test]
fn mandatory_all_finding_sources_and_absence() {
    for kind in 0..4 {
        let s = schedule(kind);
        let run = compare(&s);
        assert!(run.result.starts_with("ok:"));
        assert_eq!(
            operations(&run),
            [
                "worker_materials",
                "eprintln",
                "worker_document",
                "row.push",
                "priority_reserve"
            ]
        );
        let f = finding(&run);
        assert_eq!(
            f["kind"],
            if kind == 0 || kind == 3 {
                "failed_check"
            } else {
                "review_finding"
            }
        );
        assert_eq!(f["goal_ids"], json!([]));
        assert!(!f["report"].as_str().unwrap().is_empty());
    }
    for mut s in [schedule(0), schedule(1), schedule(2), schedule(3)] {
        s.nodes.clear();
        let run = compare(&s);
        assert_eq!(operations(&run), ["priority_reserve"]);
        assert_eq!(run.buffered, b"[]");
    }
}
#[test]
fn mandatory_reply_guards_and_checks_flags() {
    for run_match in [false, true] {
        for job_match in [false, true] {
            for generation_match in [false, true] {
                let mut s = schedule(2);
                s.nodes[0].run = run_match.then_some(9);
                s.nodes[0].job = job_match.then_some(9);
                s.nodes[0].generation = if generation_match { 9 } else { 10 };
                let r = compare(&s);
                assert_eq!(
                    r.writes,
                    usize::from((run_match || job_match) && generation_match)
                );
            }
        }
    }
    for report in [false, true] {
        for pass in [false, true] {
            for inside in [false, true] {
                let mut s = schedule(0);
                s.event = Event::Reply {
                    node: NodeId(0),
                    generation: 9,
                    reply: Reply::ChecksDone {
                        report,
                        pass,
                        in_footprint: inside,
                    },
                };
                let r = compare(&s);
                assert_eq!(r.writes, usize::from(report && (!pass || !inside)));
            }
        }
    }
    let mut s = schedule(1);
    s.detailed = true;
    assert_eq!(operations(&compare(&s)), ["priority_reserve"]);
    let mut s = schedule(2);
    s.nodes[0].phase = Phase::Reviewing;
    assert_eq!(compare(&s).writes, 0);
    let mut s = schedule(3);
    s.event = Event::Fact {
        node: NodeId(0),
        fact: Fact::CiFailed { infra: true },
    };
    assert_eq!(compare(&s).writes, 0);
    let mut s = schedule(3);
    s.nodes[0].phase = Phase::Making;
    assert_eq!(compare(&s).writes, 0);
    let s = schedule(4);
    assert_eq!(compare(&s).writes, 0);
}
#[test]
fn mandatory_material_and_document_errors() {
    for materials in [
        Read::Error,
        Read::Value(Some(json!({"goals":false}))),
        Read::Value(Some(json!({"goals":[{}]}))),
        Read::Value(Some(json!({"goals":[{"id":" \t"}]}))),
        Read::Value(Some(json!({"goals":[{"id":3}]}))),
    ] {
        let mut s = schedule(2);
        s.materials = vec![materials];
        let r = compare(&s);
        assert!(r.result.starts_with("error:"));
        assert_eq!(operations(&r), ["worker_materials"]);
    }
    for doc in [
        Read::Error,
        Read::Value(Some(json!({}))),
        Read::Value(Some(json!(null))),
    ] {
        let mut s = schedule(2);
        s.materials = vec![Read::Value(Some(json!({"goals":[]})))];
        s.documents = vec![doc];
        let r = compare(&s);
        assert!(r.result.starts_with("error:"));
        assert_eq!(operations(&r), ["worker_materials", "worker_document"]);
    }
    let mut s = schedule(2);
    s.documents = vec![Read::Error];
    let r = compare(&s);
    assert_eq!(
        operations(&r),
        ["worker_materials", "eprintln", "worker_document"]
    );
}
#[test]
fn mandatory_payloads_and_first_maker_row() {
    for nested in [false, true] {
        let mut s = schedule(2);
        let goals = json!({"goals":[{"id":" g "},{"id":"λ"},{"id":" g "}]});
        s.materials = vec![Read::Value(Some(if nested {
            json!({"maker":goals,"goals":false})
        } else {
            goals
        }))];
        s.documents = vec![Read::Value(Some(json!([{"existing":true}])))];
        s.rows = vec![
            RowChange::Delete {
                table: "maker_replies".into(),
                id: "x".into(),
            },
            put("other", "0".into(), json!({})),
            put("maker_replies", "other-node".into(), json!({"first":"λ"})),
            put("maker_replies", "0".into(), json!({"second":true})),
        ];
        let r = compare(&s);
        let f = finding(&r);
        assert_eq!(f["report"], json!({"first":"λ"}).to_string());
        assert_eq!(f["goal_ids"], json!([" g ", "λ", " g "]));
        assert_eq!(
            operations(&r),
            [
                "worker_materials",
                "worker_document",
                "row.push",
                "priority_reserve"
            ]
        );
    }
    let mut s = schedule(2);
    s.materials = vec![Read::Value(Some(
        json!({"maker":null,"goals":[{"id":"ignored"}]}),
    ))];
    assert_eq!(finding(&compare(&s))["goal_ids"], json!([]));
    for kind in [0, 3] {
        let mut s = schedule(kind);
        s.failed = vec![("test_λ".into(), "界".repeat(16020))];
        s.outside = vec!["src/λ.rs".into()];
        s.total = 9;
        s.checks = vec![(
            "ci".into(),
            "fail".into(),
            Some("https://example.invalid/log".into()),
        )];
        let f = finding(&compare(&s));
        assert_eq!(f["outside_paths_total"], 9);
        assert_eq!(f["failed_tests"][0]["test"], "test_λ");
        assert_eq!(f["outside_paths"], json!(["src/λ.rs"]));
        assert_eq!(f["report"].as_str().unwrap().chars().count(), 16000);
    }
}
fn tail(s: &str) -> &str {
    let at = s.find("        let finding = ").unwrap();
    let end = s[at..].find("\n    }\n").unwrap() + at + 7;
    &s[at..end]
}
#[test]
fn pinned_real_old_and_current_tail_and_helpers() {
    let pins: Vec<_> = include_str!("pins.txt").lines().collect();
    assert_eq!(pins[0], "f5b0ba2fb16a686232c08fe5cee907f1d43c5258");
    assert_eq!(
        format!("{:x}", Sha256::digest(include_bytes!("base-tail.rs.txt"))),
        pins[1]
    );
    assert_eq!(
        tail(include_str!("reference.rs")),
        include_str!("base-tail.rs.txt")
    );
    assert_eq!(
        tail(include_str!("current.rs")),
        tail(include_str!("../../src/lib.rs"))
    );
    let helper = include_str!("findings.rs");
    assert_eq!(format!("{:x}", Sha256::digest(helper.as_bytes())), pins[2]);
    assert_eq!(
        helper,
        include_str!("../../src/findings.rs")
            .split("pub(crate) struct CurrentReview")
            .next()
            .unwrap()
    );
}
fn generated() -> impl Strategy<Value = Schedule> {
    (0u8..5,any::<u8>(),0u8..12,0u8..6,prop::collection::vec("[^\\p{C}]{0,12}",0..4),any::<bool>()).prop_map(|(kind,flags,material,document,text,nested)| {
        let mut s=schedule(kind);
        if flags&1!=0 {s.nodes[0].run=None;s.nodes[0].job=Some(9);}
        if flags&2!=0 {s.nodes[0].generation=10;}
        if flags&4!=0 {s.detailed=true;}
        if flags&8!=0 {s.nodes[0].phase=Phase::Queued;}
        let mat=match material {
            0=>Read::Error,1=>Read::Value(None),2=>Read::Value(Some(json!({}))),3=>Read::Value(Some(json!({"goals":null}))),
            4=>Read::Value(Some(json!({"goals":[{}]}))),5=>Read::Value(Some(json!({"goals":[{"id":" "}]}))),
            _=> {let goals=json!({"goals":text.iter().map(|t|json!({"id":format!("g{t}")})).collect::<Vec<_>>()});Read::Value(Some(if nested {json!({"maker":goals})} else {goals}))}
        };
        // Independent tapes make an accidental extra read observable.
        s.materials=vec![mat,Read::Error];
        s.documents=vec![match document {0=>Read::Error,1=>Read::Value(None),2=>Read::Value(Some(json!({}))),_=>Read::Value(Some(json!([{"prior":text}])) )},Read::Error];
        s.rows=vec![put("maker_replies","0".into(),json!({"report":text}))];
        s.failed=text.iter().map(|t|(format!("test_{t}"),t.clone())).collect();
        s.outside=text.clone();s.total=text.len()+7;
        s.checks=text.iter().enumerate().map(|(i,t)|(format!("ci_{t}"),t.clone(),(i%2==0).then(||format!("https://example.invalid/{i}")))).collect();
        s
    })
}
#[test]
fn generated_tails_and_crash_cuts() {
    let output = std::env::var("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|_| std::env::var("TEST_TMPDIR"))
        .unwrap_or_else(|_| {
            std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "target".into())
        });
    std::fs::create_dir_all(&output).unwrap();
    let path = std::path::Path::new(&output).join("translate-tail-regressions.txt");
    std::fs::write(&path, include_str!("proptest-regressions.txt")).unwrap();
    let path: &'static str = Box::leak(path.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(0x004732652026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    let mut seen = [0usize; 5];
    let mut successful_populated = 0usize;
    let mut errors = 0usize;
    runner
        .run(&generated(), |s| {
            let r = compare(&s);
            if r.writes > 0 {
                let k = match s.event {
                    Event::Fact { .. } => 3,
                    Event::Reply {
                        reply: Reply::ChecksDone { .. },
                        ..
                    } => 0,
                    Event::Reply {
                        reply: Reply::ChangesRequested,
                        ..
                    } => 1,
                    _ => 2,
                };
                seen[k] += 1;
                if !s.failed.is_empty() {
                    successful_populated += 1;
                }
            } else {
                seen[4] += 1;
            }
            if r.result.starts_with("error:") {
                errors += 1;
            }
            Ok(())
        })
        .unwrap();
    assert!(
        seen.iter().all(|n| *n > 0),
        "generated branch counts {seen:?}"
    );
    assert!(successful_populated > 0 && errors > 0);
    std::eprintln!(
        "G2e: 512 schedules, branches {seen:?}, populated successes {successful_populated}, errors {errors}, seed 0x004732652026"
    );
}
