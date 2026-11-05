use super::*;
use io::{Read, Schedule, compare};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed},
};
const BASE: &str = "1111111111111111111111111111111111111111";
const HEAD: &str = "2222222222222222222222222222222222222222";
const REVIEWED: &str = "3333333333333333333333333333333333333333";
fn fixture() -> Schedule {
    let delivery: Delivery = serde_json::from_value(json!({"key":"review:0", "epoch":1,
        "output":{"Effect":{"node":0,"seq":1,"kind":{"StartRun":"Review"},"head":null}},"generation":1,
        "head_sha":HEAD,"repository":{"owner":"o","name":"r","installation":1},"issue":1,"paths":[],"attempt":1,"round":0,
        "pull_request":null,"checks":{"repository":"recorded-repository","base_sha":BASE,"plan":[]}})).unwrap();
    let snapshot = tl3_store::Snapshot {
        rows: vec![
            tl3_store::Row {
                table: "goals".into(),
                id: "0".into(),
                value: json!([{"description":"Do work","verification":"test"}]),
            },
            tl3_store::Row {
                table: "check_reports".into(),
                id: format!("0:{HEAD}"),
                value: json!({"base_sha":BASE,"results":[]}),
            },
        ],
    };
    Schedule {
        delivery,
        snapshot,
        session: None,
        tapes: BTreeMap::from([
            ("open".into(), vec![Read::Value(Value::Null)]),
            ("merge_base".into(), vec![Read::Value(json!(BASE)); 3]),
            ("diff".into(), vec![Read::Value(json!(b"diff".to_vec())); 2]),
            ("worker_materials".into(), vec![Read::Value(json!({})); 2]),
            ("worker_document".into(), vec![Read::Value(Value::Null); 3]),
            ("changed_paths".into(), vec![Read::Value(json!([]))]),
            (
                "tree".into(),
                vec![
                    Read::Value(json!(HEAD)),
                    Read::Value(json!(BASE)),
                    Read::Value(json!(HEAD)),
                ],
            ),
        ]),
    }
}
fn value(r: &io::Run) -> Value {
    serde_json::from_slice(r.result.as_ref().unwrap()).unwrap()
}
fn ops(r: &io::Run) -> Vec<&str> {
    r.trace.iter().map(|c| c["op"].as_str().unwrap()).collect()
}
#[test]
fn frozen_and_current_sources() {
    use sha2::{Digest, Sha256};
    let base = include_str!("base-review.rs.txt");
    let pins: Value = serde_json::from_str(include_str!("pins.json")).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(base)),
        pins["sha256"].as_str().unwrap()
    );
    let part = |s: &str| {
        s[s.find("pub(super) fn inputs(").unwrap()..s.find("pub(crate) fn criteria(").unwrap()]
            .trim_end()
            .to_owned()
            + "\n"
    };
    assert_eq!(
        include_str!("reference.rs"),
        format!("use super::*;\n{}", part(base))
    );
    let production = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/adapters/workers/review.rs"
    ))
    .unwrap();
    let current = include_str!("current.rs");
    assert_eq!(current, format!("use super::*;\n{}", part(&production)));
    assert!(
        base.contains(
            include_str!("criteria.rs")
                .strip_prefix("use super::*;\n")
                .unwrap()
        )
    );
    assert!(
        production.contains(
            include_str!("criteria.rs")
                .strip_prefix("use super::*;\n")
                .unwrap()
        )
    );
}
#[test]
fn mandatory_empty_exact_wire_and_trace() {
    let s = fixture();
    let r = compare(&s);
    let v = value(&r);
    assert_eq!(
        v,
        json!({"goals":[{"id":"goal-1","description":"Do work","verification":"test"}],"base_sha":BASE,"check_base_sha":BASE,"head_sha":HEAD,"diff":"diff","checks":[],"standing_rules":v["standing_rules"]})
    );
    assert_eq!(v["standing_rules"], expected()["standing_rules"]);
    assert_eq!(
        ops(&r),
        [
            "open",
            "merge_base",
            "worker_materials",
            "diff",
            "worker_document",
            "worker_document",
            "worker_document",
            "validate_inputs"
        ]
    );
    assert_eq!(r.trace[7]["args"], json!(["reviewer", v]));
    assert_eq!(r.trace[4]["args"], json!(["0", "review_fixes"]));
    assert_eq!(r.trace[5]["args"], json!(["0", "maker_replies"]));
    assert_eq!(r.trace[6]["args"], json!(["0", "maker_replies"]));
}
proptest! {
    #![proptest_config(Config {cases:64,rng_seed:RngSeed::Fixed(0x17_0730_2026),failure_persistence:persistence(),..Config::default()})]
    #[test]
    fn generated_read_tapes(flags in any::<u16>(), fail in 0usize..24, classes in prop::collection::vec(0u8..8,0..20)) {
        let mut s=fixture();
        if flags&1 != 0 {s.delivery.round=1;s.session=Some(json!({"head_sha":REVIEWED}));}
        if flags&2 != 0 {s.delivery.checks.as_mut().unwrap().reviewed_sha=Some(CommitId::new(REVIEWED).unwrap());}
        s.snapshot.rows[1].value["selected_failures"]=json!(["test_a.py::test_a","test_b.py",null]);
        s.tapes.insert("changed_paths".into(),vec![Read::Value(json!(["test_a.py"]))]);
        if flags&4 != 0 { s.snapshot.rows[1].value["results"]=json!([{"forcing":null}]); }
        if flags&8 != 0 {s.tapes.get_mut("worker_materials").unwrap()[1]=Read::Value(json!({"issue":{"title":"é".repeat(1001),"body":"界".repeat(16001)}}));}
        s.tapes.insert("worker_document".into(),vec![Read::Value(json!({"head_sha":HEAD,"fixes":[]})),Read::Value(json!({"head":HEAD,"test_evidence":[{"base":true,"tree":BASE},{"tree":HEAD},{"tree":"wrong"}]})),Read::Value(json!({"head":HEAD,"generation_evidence":{"output_tree":HEAD}}))]);
        let keys=["open","merge_base","worker_materials","diff","changed_paths","worker_document","tree","validate_inputs"];
        if fail<keys.len()*3 {
            let key=keys[fail/3]; let at=fail%3;
            let tape=s.tapes.entry(key.into()).or_default(); while tape.len()<=at {tape.push(Read::Value(Value::Null));}
            let class=classes.first().copied().unwrap_or(0);
            if class>0 {tape[at]=match class {1=>Read::Io,2=>Read::Json,3=>Read::Git,4=>Read::Invalid,5=>Read::Store,_=>Read::Worktree(class)};}
        }
        compare(&s);
    }
}

fn persistence() -> Option<Box<dyn proptest::test_runner::FailurePersistence>> {
    let dir = std::env::var("TEST_UNDECLARED_OUTPUTS_DIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let path = Box::leak(format!("{dir}/review-inputs-regressions.txt").into_boxed_str());
    let target = Path::new(path);
    if !target.exists() {
        std::fs::write(target, include_str!("proptest-regressions.txt")).unwrap();
    }
    Some(Box::new(FileFailurePersistence::Direct(path)))
}

fn expected() -> Value {
    serde_json::from_str(include_str!("empty-input.json")).unwrap()
}
fn assert_wire(s: &Schedule, v: Value, expected_ops: &[&str]) -> io::Run {
    let r = compare(s);
    assert_eq!(r.result.as_ref().unwrap(), &serde_json::to_vec(&v).unwrap());
    assert_eq!(ops(&r), expected_ops);
    // Check all arguments and responses against the schedule, not just op names.
    let mut counts = BTreeMap::<&str, usize>::new();
    for call in &r.trace {
        let op = call["op"].as_str().unwrap();
        let at = counts.entry(op).or_default();
        let response = s
            .tapes
            .get(op)
            .and_then(|t| t.get(*at))
            .cloned()
            .unwrap_or(Read::Value(Value::Null));
        *at += 1;
        if let Read::Value(v) = response {
            assert_eq!(call["outcome"], json!({"ok":v}));
        }
        match op {
            "open" => assert_eq!(
                call["args"],
                json!(["recorded-repository", "recorded-repository"])
            ),
            "worker_materials" => assert_eq!(call["args"], json!(["0"])),
            "worker_document" => {
                let kind = ["review_fixes", "maker_replies", "maker_replies"][*at - 1];
                assert_eq!(call["args"], json!(["0", kind]));
            }
            "validate_inputs" => assert_eq!(call["args"], json!(["reviewer", v])),
            _ => {}
        }
    }
    r
}
const BASIC: &[&str] = &[
    "open",
    "merge_base",
    "worker_materials",
    "diff",
    "worker_document",
    "worker_document",
    "worker_document",
    "validate_inputs",
];
fn reviewed() -> Schedule {
    let mut s = fixture();
    s.delivery.round = 1;
    s.session = Some(json!({"head_sha":REVIEWED}));
    s
}

#[test]
fn mandatory_diff_bounds_and_utf8_at_both_reads() {
    for scope in [false, true] {
        for (bytes, error) in [
            (vec![], None),
            (vec![b'x'; 524288], None),
            (vec![b'x'; 524289], Some("exceeds the input bound")),
            (vec![255], Some("must be UTF-8")),
        ] {
            let mut s = if scope { reviewed() } else { fixture() };
            let at = usize::from(scope);
            s.tapes.get_mut("diff").unwrap()[at] = Read::Value(json!(bytes));
            let r = compare(&s);
            if let Some(error) = error {
                assert_eq!(
                    r.result,
                    Err(format!(
                        "{} diff {error}",
                        if scope { "scope" } else { "review" }
                    ))
                );
                assert_eq!(
                    ops(&r),
                    if scope {
                        vec![
                            "open",
                            "merge_base",
                            "worker_materials",
                            "diff",
                            "merge_base",
                            "diff",
                        ]
                    } else {
                        vec!["open", "merge_base", "worker_materials", "diff"]
                    }
                );
            } else {
                let mut v = expected();
                let text = String::from_utf8(bytes).unwrap();
                let calls = if scope {
                    v["base_sha"] = json!(REVIEWED);
                    if text != "diff" {
                        v["scope_diff"] = json!(text);
                    }
                    vec![
                        "open",
                        "merge_base",
                        "worker_materials",
                        "diff",
                        "merge_base",
                        "diff",
                        "worker_document",
                        "worker_document",
                        "worker_document",
                        "validate_inputs",
                    ]
                } else {
                    v["diff"] = json!(text);
                    BASIC.to_vec()
                };
                assert_wire(&s, v, &calls);
            }
        }
    }
}

#[test]
fn mandatory_base_precedence_scope_and_reset() {
    for (round, session, reviewed_head) in [
        (0, false, false),
        (0, true, true),
        (1, false, true),
        (1, true, false),
        (1, true, true),
        (2, true, true),
    ] {
        let mut s = fixture();
        s.delivery.round = round;
        s.session = session.then(|| {
            if reviewed_head {
                json!({})
            } else {
                json!({"head_sha":REVIEWED})
            }
        });
        s.delivery.checks.as_mut().unwrap().reviewed_sha =
            reviewed_head.then(|| CommitId::new(REVIEWED).unwrap());
        let use_old = round == 1 && session;
        let mut v = expected();
        let calls = if use_old {
            v["base_sha"] = json!(REVIEWED);
            vec![
                "open",
                "merge_base",
                "worker_materials",
                "diff",
                "merge_base",
                "diff",
                "worker_document",
                "worker_document",
                "worker_document",
                "validate_inputs",
            ]
        } else {
            BASIC.to_vec()
        };
        let r = assert_wire(&s, v, &calls);
        assert_eq!(
            r.trace[1]["args"],
            if use_old {
                json!([REVIEWED, BASE])
            } else {
                json!([BASE, HEAD])
            }
        );
        if use_old {
            assert_eq!(r.trace[3]["args"], json!([REVIEWED, HEAD]));
            assert_eq!(r.trace[4]["args"], json!([BASE, HEAD]));
        }
    }
    let mut s = reviewed();
    s.tapes.get_mut("merge_base").unwrap()[0] = Read::Value(json!(REVIEWED));
    let r = assert_wire(
        &s,
        expected(),
        &[
            "open",
            "merge_base",
            "merge_base",
            "worker_materials",
            "diff",
            "worker_document",
            "worker_document",
            "worker_document",
            "validate_inputs",
        ],
    );
    assert_eq!(r.trace[2]["args"], json!([BASE, HEAD]));
    let mut s = reviewed();
    s.tapes.get_mut("diff").unwrap()[1] = Read::Value(json!(b"scope".to_vec()));
    let mut v = expected();
    v["base_sha"] = json!(REVIEWED);
    v["scope_diff"] = json!("scope");
    assert_wire(
        &s,
        v,
        &[
            "open",
            "merge_base",
            "worker_materials",
            "diff",
            "merge_base",
            "diff",
            "worker_document",
            "worker_document",
            "worker_document",
            "validate_inputs",
        ],
    );
}

fn populated(count: usize) -> (Schedule, Value) {
    let mut s = fixture();
    let mut v = expected();
    let failures: Vec<_> = (0..count)
        .map(|i| format!("test_{i}.py::test_case"))
        .collect();
    s.snapshot.rows[1].value["selected_failures"] = json!(failures);
    let touched: Vec<_> = (0..count)
        .filter(|i| i % 2 == 0)
        .map(|i| format!("test_{i}.py"))
        .collect();
    s.tapes
        .insert("changed_paths".into(), vec![Read::Value(json!(touched))]);
    let tests:Vec<_>=(0..count).map(|i|json!({"test":format!("test_{i}.py"),"head":(["passed","assertion","error","no-report","filtered",""][i%6])})).collect();
    let results = json!([{"name":"Other","tests":[{"test":"test_3.py","head":"passed"}]},{"name":"Existing tests that import the change","tests":tests}]);
    s.snapshot.rows[1].value["results"] = results.clone();
    v["checks"] = results;
    let edited: Vec<_> = failures
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, f)| f)
        .collect();
    let unresolved: Vec<_> = failures
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 6 >= 3)
        .map(|(_, f)| f)
        .collect();
    if !edited.is_empty() {
        v["edited_selected_tests"] = json!(edited);
    }
    if !unresolved.is_empty() {
        v["unresolved_displaced_failures"] = json!(unresolved);
    }
    (s, v)
}
#[test]
fn mandatory_collections_are_uncapped_ordered_and_keep_duplicates() {
    // There is no collection cap. 20 is a nearby findings limit, not an input limit.
    for n in [0, 1, 20, 21, 101] {
        let (s, v) = populated(n);
        let r = assert_wire(
            &s,
            v,
            &[
                "open",
                "merge_base",
                "worker_materials",
                "diff",
                "changed_paths",
                "worker_document",
                "worker_document",
                "worker_document",
                "validate_inputs",
            ],
        );
        assert_eq!(r.trace[4]["args"], json!([BASE, HEAD]));
    }
    let (mut s, mut v) = populated(0);
    s.snapshot.rows[1].value["selected_failures"] = json!(["test.py::a", null, "test.py::a", 42]);
    s.tapes.insert(
        "changed_paths".into(),
        vec![Read::Value(json!(["test.py"]))],
    );
    v["edited_selected_tests"] = json!(["test.py::a", "test.py::a"]);
    v["unresolved_displaced_failures"] = json!(["test.py::a", "test.py::a"]);
    assert_wire(
        &s,
        v,
        &[
            "open",
            "merge_base",
            "worker_materials",
            "diff",
            "changed_paths",
            "worker_document",
            "worker_document",
            "worker_document",
            "validate_inputs",
        ],
    );
}

#[test]
fn mandatory_issue_character_prefix_and_presence_not_truthiness() {
    for field in ["forcing", "proof", "declared_symptom"] {
        for n in [0, 1000, 1001, 16000, 16001] {
            let mut s = fixture();
            let mut v = expected();
            let results = json!([{field:null}]);
            s.snapshot.rows[1].value["results"] = results.clone();
            v["checks"] = results;
            s.tapes.get_mut("worker_materials").unwrap()[1] =
                Read::Value(json!({"issue":{"title":"é".repeat(n),"body":"界".repeat(n)}}));
            v["issue"] = json!({"title":"é".repeat(n.min(1000)),"body":"界".repeat(n.min(16000))});
            assert_wire(
                &s,
                v,
                &[
                    "open",
                    "merge_base",
                    "worker_materials",
                    "diff",
                    "worker_materials",
                    "worker_document",
                    "worker_document",
                    "worker_document",
                    "validate_inputs",
                ],
            );
        }
        for materials in [
            Value::Null,
            json!({}),
            json!({"issue":null}),
            json!({"issue":{"title":5,"body":false}}),
        ] {
            let mut s = fixture();
            let mut v = expected();
            let results = json!([{field:{}}]);
            s.snapshot.rows[1].value["results"] = results.clone();
            v["checks"] = results;
            s.tapes.get_mut("worker_materials").unwrap()[1] = Read::Value(materials.clone());
            if materials.get("issue").is_some() {
                v["issue"] = json!({"title":"","body":""});
            }
            assert_wire(
                &s,
                v,
                &[
                    "open",
                    "merge_base",
                    "worker_materials",
                    "diff",
                    "worker_materials",
                    "worker_document",
                    "worker_document",
                    "worker_document",
                    "validate_inputs",
                ],
            );
        }
    }
}

#[test]
fn mandatory_separate_reply_reads_and_opaque_receipt_shapes() {
    for n in [0, 1, 20, 21, 101] {
        let mut s = fixture();
        let mut v = expected();
        let evidence:Vec<_>=(0..n).map(|i|json!({"base":i%2==0,"tree":if i%2==0 {BASE} else {HEAD},"tests_identity":{"sha256":"digest","count":1001,"prefix":["test"]},"red_message_matches":true,"groups":[{"totals":{"passed":25,"failed":1,"errors":267}}]})).collect();
        s.tapes.insert("worker_document".into(),vec![Read::Value(json!({"head_sha":HEAD,"fixes":["fix"]})),Read::Value(json!({"head":HEAD,"test_evidence":evidence})),Read::Value(json!({"head":HEAD,"generation_evidence":{"output_tree":HEAD,"receipt":"second read"}}))]);
        v["review_fixes"] = json!({"fixes":["fix"]});
        if n > 0 {
            v["test_evidence"] = json!(evidence);
        }
        v["generation_evidence"] = json!({"output_tree":HEAD,"receipt":"second read"});
        let r = assert_wire(
            &s,
            v,
            &[
                "open",
                "merge_base",
                "worker_materials",
                "diff",
                "worker_document",
                "worker_document",
                "tree",
                "tree",
                "worker_document",
                "tree",
                "validate_inputs",
            ],
        );
        assert_eq!(r.trace[6]["args"], json!([HEAD]));
        assert_eq!(r.trace[7]["args"], json!([BASE]));
        assert_eq!(r.trace[9]["args"], json!([HEAD]));
    }
    for profile in 0..8 {
        let mut s = fixture();
        let mut v = expected();
        let first = match profile {
            0 => json!(null),
            1 => json!({"head":BASE,"test_evidence":[]}),
            2 => json!({"head":HEAD}),
            3 => json!({"head":HEAD,"test_evidence":{}}),
            _ => {
                json!({"head":HEAD,"test_evidence":[{"tree":"wrong"},{"tree":BASE,"base":false},{"tree":HEAD,"base":true},{"tree":HEAD,"base":"true"}]})
            }
        };
        let second = match profile {
            0 => json!({"head":HEAD,"generation_evidence":{"output_tree":HEAD}}),
            1 => json!({"head":BASE,"generation_evidence":{"output_tree":HEAD}}),
            2 => json!({"head":HEAD}),
            3 => json!({"head":HEAD,"generation_evidence":null}),
            _ => json!({"head":HEAD,"generation_evidence":{"output_tree":"wrong"}}),
        };
        s.tapes.insert(
            "worker_document".into(),
            vec![
                Read::Value(json!({"head_sha":BASE,"stale":true})),
                Read::Value(first),
                Read::Value(second),
            ],
        );
        if profile >= 4 {
            v["test_evidence"] = json!([{"tree":HEAD,"base":"true"}]);
        }
        if profile == 0 {
            v["generation_evidence"] = json!({"output_tree":HEAD});
        }
        let mut calls = vec![
            "open",
            "merge_base",
            "worker_materials",
            "diff",
            "worker_document",
            "worker_document",
        ];
        if profile >= 4 {
            calls.extend(["tree", "tree"]);
        }
        calls.push("worker_document");
        if profile == 0 || profile >= 3 {
            calls.push("tree");
        }
        calls.push("validate_inputs");
        assert_wire(&s, v, &calls);
    }
}

#[test]
fn mandatory_all_recorded_error_classes_at_every_reached_read() {
    for scope in [false, true] {
        let mut s = fixture();
        s.snapshot.rows[1].value["results"] = json!([{"forcing":null}]);
        s.tapes.insert(
            "worker_materials".into(),
            vec![Read::Value(json!({})), Read::Value(json!({"issue":{}}))],
        );
        s.tapes.insert(
            "worker_document".into(),
            vec![
                Read::Value(Value::Null),
                Read::Value(json!({"head":HEAD,"test_evidence":[]})),
                Read::Value(json!({"head":HEAD,"generation_evidence":{"output_tree":HEAD}})),
            ],
        );
        s.snapshot.rows[1].value["selected_failures"] = json!([]);
        if scope {
            s.delivery.round = 1;
            s.session = Some(json!({"head_sha":REVIEWED}));
        }
        let good = compare(&s);
        let mut counts = BTreeMap::<String, usize>::new();
        for (cut, call) in good.trace.iter().enumerate() {
            let op = call["op"].as_str().unwrap();
            let at = *counts.entry(op.into()).or_default();
            *counts.get_mut(op).unwrap() += 1;
            for class in [
                Read::Io,
                Read::Json,
                Read::Git,
                Read::Invalid,
                Read::Store,
                Read::Worktree(0),
                Read::Worktree(1),
                Read::Worktree(2),
                Read::Worktree(3),
                Read::Worktree(4),
                Read::Worktree(5),
                Read::Worktree(6),
                Read::Worktree(7),
            ] {
                let mut broken = s.clone();
                let tape = broken.tapes.entry(op.into()).or_default();
                while tape.len() <= at {
                    tape.push(Read::Value(Value::Null));
                }
                tape[at] = class;
                let r = compare(&broken);
                assert!(r.result.is_err());
                assert_eq!(r.trace.len(), cut + 1);
                assert_eq!(&r.trace[..cut], &good.trace[..cut]);
                assert_eq!(r.trace[cut]["args"], call["args"]);
                assert!(r.trace[cut]["outcome"].get("error").is_some());
            }
        }
    }
}

#[test]
fn mandatory_boundary_errors_stop_at_original_point() {
    for mode in 0..12 {
        let mut s = fixture();
        let (message, calls): (&str, Vec<&str>) = match mode {
            0 => {
                s.delivery.checks = None;
                ("review needs its check plan", vec![])
            }
            1 => {
                s.delivery.head_sha = None;
                ("review needs its exact head", vec![])
            }
            2 => {
                s.snapshot.rows.pop();
                (
                    "review needs exact-head check evidence",
                    vec!["open", "merge_base"],
                )
            }
            3 => {
                s.snapshot.rows[1].value["base_sha"] = json!(HEAD);
                (
                    "review and checks disagree on base",
                    vec!["open", "merge_base"],
                )
            }
            4 => {
                s.snapshot.rows.remove(0);
                ("review needs goals", vec!["open", "merge_base"])
            }
            5 => {
                s.tapes.get_mut("worker_materials").unwrap()[0] = Read::Value(Value::Null);
                (
                    "review materials missing",
                    vec!["open", "merge_base", "worker_materials"],
                )
            }
            6 => {
                s.tapes.get_mut("worker_materials").unwrap()[0] =
                    Read::Value(json!({"standing_rules":{}}));
                (
                    "standing rules must be a list",
                    vec!["open", "merge_base", "worker_materials"],
                )
            }
            7 => {
                s = reviewed();
                s.session = Some(json!({}));
                ("reviewer session has no reviewed head", vec![])
            }
            8 => {
                s.snapshot.rows[0].value = json!({});
                ("goals must be a list", vec!["open", "merge_base"])
            }
            9 => {
                s.snapshot.rows[0].value = json!([null]);
                ("goal must be an object", vec!["open", "merge_base"])
            }
            10 => {
                s.snapshot.rows[0].value = json!([{"id":""}]);
                ("goal needs an id", vec!["open", "merge_base"])
            }
            _ => {
                s.snapshot.rows[0].value = json!([{"id":"x"},{"id":"x"}]);
                ("goal ids must be unique", vec!["open", "merge_base"])
            }
        };
        let r = compare(&s);
        assert_eq!(r.result, Err(message.into()));
        assert_eq!(ops(&r), calls);
    }
}

proptest! {
    #![proptest_config(Config {cases:48,rng_seed:RngSeed::Fixed(0x17_0730_2026),failure_persistence:persistence(),..Config::default()})]
    #[test]
    fn generated_bounding_and_optional_shapes(size in prop::sample::select(vec![0usize,1,20,21,101]), bound in 0usize..4, bits in any::<u16>(), outcome in 0usize..8) {
        let (mut s,_) = populated(size);
        let chars=[0,1000,16000,16001][bound];
        if bits&1 != 0 {s.delivery.round=1;s.session=Some(if bits&2!=0 {json!({"head_sha":REVIEWED})} else {json!({})});}
        if bits&4 != 0 {s.delivery.checks.as_mut().unwrap().reviewed_sha=Some(CommitId::new(REVIEWED).unwrap());}
        s.tapes.get_mut("merge_base").unwrap()[0]=Read::Value(json!(if bits&8 != 0 {REVIEWED} else {BASE}));
        let key=["absent","forcing","proof","declared_symptom"][bound];
        s.snapshot.rows[1].value["results"].as_array_mut().unwrap().push(json!({key:null}));
        s.tapes.get_mut("worker_materials").unwrap()[1]=Read::Value(if bits&16!=0 {json!({"issue":{"title":"é".repeat(chars),"body":"界".repeat(chars)}})} else {json!({})});
        let first=if bits&32!=0 {json!({"head":HEAD,"test_evidence":[{"tree":HEAD,"base":false},{"tree":BASE,"base":true},{"tree":HEAD,"base":true}]})} else {json!({"head":BASE,"test_evidence":[]})};
        let second=if bits&64!=0 {json!({"head":HEAD,"generation_evidence":{"output_tree":if bits&128!=0 {HEAD} else {BASE}}})} else {Value::Null};
        s.tapes.insert("worker_document".into(),vec![Read::Value(if bits&256!=0{json!({"head_sha":HEAD,"findings":[]})}else{Value::Null}),Read::Value(first),Read::Value(second)]);
        let bytes=match outcome {0=>vec![],1=>vec![b'x';524288],2=>vec![b'x';524289],3=>vec![255],_=>b"changed diff".to_vec()};
        s.tapes.get_mut("diff").unwrap()[usize::from(bits&512 != 0)]=Read::Value(json!(bytes));
        if bits&1024!=0 {s.snapshot.rows[1].value["selected_failures"]=Value::Null;}
        compare(&s);
    }
}

#[test]
fn mandatory_invalid_commit_ids_before_any_read() {
    let mut s = fixture();
    s.delivery.head_sha = Some("invalid".into());
    let r = compare(&s);
    assert!(r.result.unwrap_err().contains("invalid_commit"));
    assert!(r.trace.is_empty());
    let mut s = reviewed();
    s.session = Some(json!({"head_sha":"invalid"}));
    let r = compare(&s);
    assert!(r.result.unwrap_err().contains("invalid_commit"));
    assert!(r.trace.is_empty());
}
