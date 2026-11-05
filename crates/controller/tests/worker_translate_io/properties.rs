use super::*;
use io::{NEW_HEAD, OLD_HEAD, Read, Schedule, compare, schedule};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};
use sha2::{Digest, Sha256};
fn arm(source: &str) -> &str {
    let a = source
        .find("            Observation::WorkerFailed {")
        .unwrap();
    let b = source[a..]
        .find("            Observation::Plan {")
        .map_or(source.len(), |b| a + b);
    &source[a..b]
}
#[test]
fn production_and_reference_sources_are_pinned() {
    let pins: Value = serde_json::from_str(include_str!("pins.json")).unwrap();
    let base = include_str!("base-arms.rs.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(base)),
        pins["blobs"]["arms"]["sha256"]
    );
    let source = include_str!("../../src/lib.rs");
    for (copy, expected) in [
        (include_str!("reference.rs"), base),
        (include_str!("current.rs"), arm(source)),
    ] {
        let a = copy
            .find("            Observation::WorkerFailed {")
            .unwrap();
        let b = copy[a..].find("        };\n        *detail").unwrap() + a;
        assert_eq!(&copy[a..b], expected);
    }
    // Frozen helper blobs are separate from the two independently compiled arms.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name, pin) in pins["blobs"].as_object().unwrap() {
        if name == "arms" {
            continue;
        }
        let frozen = std::fs::read(root.join(format!(
            "crates/controller/tests/worker_translate_io/base-{name}.rs.txt"
        )))
        .unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&frozen)), pin["sha256"]);
        let production = std::fs::read_to_string(root.join(pin["path"].as_str().unwrap())).unwrap();
        let compiled = std::fs::read_to_string(root.join(format!(
            "crates/controller/tests/worker_translate_io/{name}.rs"
        )))
        .unwrap();
        // A leaf whose production body changed after the freeze has a separate
        // current copy: the frozen parts then pin only the reference copy, and
        // the current copy must equal the production body before its tests.
        let current = root.join(format!(
            "crates/controller/tests/worker_translate_io/{name}-current.rs"
        ));
        let changed = current.exists();
        if changed {
            let copy = std::fs::read_to_string(&current).unwrap();
            let body = production.split("#[cfg(test)]").next().unwrap();
            assert_eq!(copy.trim_end(), body.trim_end(), "current leaf drift: {name}");
        }
        for part in pin["parts"].as_array().unwrap() {
            let part = part.as_str().unwrap();
            assert!(changed || production.contains(part), "production leaf drift: {name}");
            let normalized = compiled.replace("pub(super) fn", "fn");
            assert!(
                compiled.contains(part) || normalized.contains(part),
                "compiled leaf drift: {name}"
            );
        }
    }
}
fn forecast() -> Value {
    json!({"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"safe.rs":0},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true})
}
fn reply(role: &str) -> Value {
    match role {
        "ingest" => {
            json!({"kind":"brief","brief":"Do work","goals":[{"description":"Do work","verification":"test"}],"memory_ids":[],"files":["safe.rs"],"forecast":forecast()})
        }
        "audit" => json!({"verdict":"agree","objections":[]}),
        "planner" => {
            json!({"kind":"leaf","files":["safe.rs"],"checks":[{"kind":"regression","node_ids":["test_safe.py::test_safe"],"goal_ids":[0]}],"no_design_questions":true,"lands_on_its_own":true,"violations":[],"forecast":forecast()})
        }
        "maker" => {
            json!({"kind":"done","head":NEW_HEAD,"self_check":[{"goal_id":"goal-1","status":"met","evidence":"passed"}]})
        }
        _ => json!({"verdict":"approve","head_sha":OLD_HEAD,"blocking_findings":[],"findings":[]}),
    }
}
fn routed(s: &Schedule) -> io::Run {
    let r = compare(s);
    assert!(r.result.get("event").is_some(), "{}", r.result);
    r
}
#[test]
fn mandatory_roles_and_staleness() {
    for role in ["ingest", "audit", "planner", "maker", "reviewer"] {
        let s = schedule(role, reply(role));
        let r = routed(&s);
        assert!(
            !r.result.to_string().contains("RunFailed"),
            "{role}: {}",
            r.result
        );
        for stale in 0..3 {
            let mut s = s.clone();
            match stale {
                0 => s.run = None,
                1 => s.run = Some(6),
                _ => s.generation = 6,
            };
            let r = compare(&s);
            assert!(r.calls.is_empty());
            assert_eq!(r.result["event"], Value::Null);
        }
    }
    for role in ["writer", "classifier", "postmortem", "unknown"] {
        let r = routed(&schedule(role, reply("audit")));
        assert!(r.result.to_string().contains("RunFailed"));
        assert!(r.calls.is_empty());
    }
}
#[test]
fn mandatory_role_validation_and_error_classes() {
    for role in ["ingest", "audit", "planner", "maker", "reviewer"] {
        let mut s = schedule(role, reply(role));
        s.phase = tl3_kernel::Phase::Queued;
        assert!(routed(&s).result.to_string().contains("RunFailed"));
        assert!(
            routed(&schedule(role, json!({})))
                .result
                .to_string()
                .contains("RunFailed")
        );
        let mut s = schedule(role, reply(role));
        s.missing = true;
        assert!(
            compare(&s)
                .result
                .to_string()
                .contains("worker node missing")
        );
    }
    for read in [
        Read::Store,
        Read::Json,
        Read::Value(None),
        Read::Value(Some(json!({"standing_rules":false}))),
    ] {
        let mut s = schedule("reviewer", reply("reviewer"));
        s.materials = vec![read];
        assert!(compare(&s).result.get("error").is_some());
    }
    for read in [
        Read::Store,
        Read::Json,
        Read::Value(None),
        Read::Value(Some(json!({}))),
    ] {
        let mut s = schedule("maker", reply("maker"));
        s.materials = vec![read];
        assert!(compare(&s).result.get("error").is_some());
    }
    for role in ["ingest", "audit", "planner"] {
        let mut s = schedule(role, reply(role));
        s.protected = vec![2];
        assert!(compare(&s).result.get("error").is_some());
    }
}
fn blockers(count: usize, size: usize) -> Value {
    json!((0..count).map(|i|json!({"citation":{"kind":if i%2==0 {"goal"} else {"standing_rule"},"id":if i%2==0 {"goal-1"}else{"test-report-integrity"}},"problem":"é".repeat(size),"evidence":"Proof"})).collect::<Vec<_>>())
}
#[test]
fn mandatory_findings_budget_and_validation() {
    let mut s = schedule(
        "reviewer",
        json!({"verdict":"changes","blocking_findings":blockers(20,3000),"findings":[]}),
    );
    let r = routed(&s);
    assert_eq!(r.result["detailed_review"], true);
    let row = r
        .rows
        .iter()
        .find(|r| r["Put"]["table"] == "maker_findings")
        .unwrap();
    let items = row["Put"]["value"].as_array().unwrap();
    assert!(!items.is_empty() && items.len() < 20);
    assert!(
        items
            .iter()
            .map(|i| i["report"].as_str().unwrap().chars().count())
            .sum::<usize>()
            <= 16000
    );
    if let Observation::WorkerReply { reply, .. } = &mut s.observation {
        reply["blocking_findings"][0]["citation"]["id"] = json!("unknown");
    }
    assert!(routed(&s).result.to_string().contains("RunFailed"));
    let mut s = schedule("reviewer", reply("reviewer"));
    if let Observation::WorkerReply { reply, .. } = &mut s.observation {
        reply["head_sha"] = json!(NEW_HEAD);
    }
    assert!(routed(&s).result.to_string().contains("RunFailed"));
}
#[test]
fn mandatory_maker_self_check_and_intern_crashes() {
    let valid = schedule("maker", reply("maker"));
    assert!(routed(&valid).writes > 0);
    for kind in 0..5 {
        let mut s = valid.clone();
        if let Observation::WorkerReply { reply, .. } = &mut s.observation {
            match kind {
                0 => reply["self_check"][0]["goal_id"] = json!("unknown"),
                1 => {
                    let check = reply["self_check"][0].clone();
                    reply["self_check"].as_array_mut().unwrap().push(check);
                }
                2 => {
                    reply["self_check"][0] =
                        json!({"goal_id":"goal-1","status":"pending","why":"owned elsewhere"});
                }
                3 => {
                    reply["self_check"][0] =
                        json!({"goal_id":"goal-1","status":"not_met","why":"not met"});
                }
                _ => {
                    reply["reproductions"] =
                        json!([{"finding_id":"missing","reason":"not applicable"}])
                }
            }
        }
        let r = routed(&s);
        assert!(
            r.result.to_string().contains(if kind == 3 {
                "MakerCantMeet"
            } else {
                "RunFailed"
            }),
            "{}",
            r.result
        );
    }
    for failure in [false, true] {
        let mut s = valid.clone();
        s.intern_failures = vec![failure];
        compare(&s);
    }
    let mut s = schedule(
        "maker",
        json!({"kind":"needs_files","files":[{"path":"one.rs","why":"needed"},{"path":"two.rs","why":"needed"}]}),
    );
    let r = routed(&s);
    assert_eq!(r.writes, 3);
    assert!(r.result.to_string().contains("MakerNeedsFiles"));
    s.intern_failures = vec![false, true];
    assert!(compare(&s).result.get("error").is_some());
}
#[test]
fn mandatory_worker_failed_classes() {
    for class in [
        "infrastructure",
        "validation",
        "transport",
        "quota",
        "timeout",
        "blocked",
        "unknown",
        "",
    ] {
        let mut s = schedule("audit", Value::Null);
        s.observation = Observation::WorkerFailed {
            node: NodeId(0),
            generation: 7,
            failure_class: class.into(),
            failure_message: format!("safe {class}"),
        };
        let r = routed(&s);
        assert!(r.result.to_string().contains("RunFailed"));
        assert_eq!(r.rows.len(), 1);
        s.run = None;
        assert!(compare(&s).calls.is_empty());
        s.run = Some(7);
        s.generation = 6;
        assert!(compare(&s).calls.is_empty());
    }
}
#[test]
fn fixed_seed_changing_reads_and_optional_fields() {
    let config = Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(0x7602b2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "crates/controller/tests/worker_translate_io/proptest-regressions.txt",
        ))),
        ..Config::default()
    };
    TestRunner::new(config).run(&(0usize..5,0u8..16,any::<bool>(),prop::collection::vec(0u8..8,0..12),prop::collection::vec(any::<bool>(),0..8)),|(role,bits,review_changes,tape,failures)|{
        let role=["ingest","audit","planner","maker","reviewer"][role];
        let mut s=schedule(role,reply(role));s.intern_failures=failures;
        s.documents=tape.iter().map(|v|match v {0=>Read::Store,1=>Read::Json,2=>Read::Value(Some(json!({}))),3=>Read::Value(Some(json!([{"kind":"review_finding","id":"f","reviewed_sha":OLD_HEAD,"report":"fix","goal_ids":["goal-1"]}]))),_=>Read::Value(None)}).collect();
        if bits&1!=0 {s.head=None;}
        if bits&2!=0{s.round=1;}
        if bits&4!=0{s.run=None;}
        if let Observation::WorkerReply{reply,model,..}=&mut s.observation {
            if bits&8!=0 {*model=None;}
            if role=="reviewer"&&review_changes {*reply=json!({"verdict":"changes","blocking_findings":blockers(1+tape.len(),1+usize::from(bits)*200),"findings":[]});}
        }
        compare(&s);Ok(())
    }).unwrap();
}
fn reproduction(tests: bool) -> Schedule {
    let mut s = schedule("maker", reply("maker"));
    s.round = 1;
    s.documents = vec![Read::Value(Some(
        json!([{"kind":"review_finding","id":"f","reviewed_sha":OLD_HEAD,"report":"Fix the case","goal_ids":["goal-1"]}]),
    ))];
    if let Observation::WorkerReply { reply, .. } = &mut s.observation {
        reply["reproductions"] = if tests {
            json!([{"finding_id":"f","tests":["test_safe.py::test_safe"],"proof":{"subjects":[],"observer_support":[],"red_message":"fails before fix"}}])
        } else {
            json!([{"finding_id":"f","reason":"documented review only"}])
        };
    }
    s
}
fn plan(tag: &str) -> Value {
    json!({"repository":format!("/safe/{tag}"),"base_sha":OLD_HEAD,"reviewed_sha":OLD_HEAD,"reproducing_unavailable":"safe optional evidence","selected_failures":["test_original.py::test_original"],"plan":[{"name":tag,"kind":"regression","tests":["test_safe.py::test_safe"]},{"name":"old reproduction","kind":"reproducing","tests":["test_old.py::test_old"]}]})
}
#[test]
fn mandatory_reproductions_and_eager_second_plan_read() {
    for tests in [false, true] {
        for present in [false, true] {
            let mut s = reproduction(tests);
            s.plans = vec![
                Read::Value(present.then(|| plan("first"))),
                Read::Value(Some(plan("second"))),
            ];
            let r = routed(&s);
            assert!(r.result.to_string().contains("MakerDone"), "{}", r.result);
            assert_eq!(
                r.calls.iter().filter(|v| v["op"] == "check_run").count(),
                2,
                "Option::or eagerly reads both plans"
            );
            assert!(r.rows.iter().any(|r| r["Put"]["table"] == "review_fixes"));
            let selected = r
                .rows
                .iter()
                .find(|r| r["Put"]["table"] == "check_plans")
                .unwrap();
            assert_eq!(
                selected["Put"]["value"]["repository"],
                format!("/safe/{}", if present { "first" } else { "second" })
            );
            assert_eq!(
                selected["Put"]["value"].get("reviewed_sha").is_some(),
                tests
            );
        }
    }
    for bad in [
        Read::Store,
        Read::Json,
        Read::Value(Some(json!({}))),
        Read::Value(None),
    ] {
        let mut s = reproduction(true);
        s.plans = vec![Read::Value(None), bad];
        assert!(compare(&s).result.get("error").is_some());
    }
    let mut s = reproduction(true);
    s.plans = vec![Read::Value(Some(plan("first"))), Read::Store];
    assert!(
        compare(&s).result.get("error").is_some(),
        "second failure propagates despite first plan"
    );
    let mut s = reproduction(true);
    s.plans = vec![Read::Value(Some(plan("first"))), Read::Value(None)];
    s.intern_failures = vec![true];
    let r = compare(&s);
    assert!(r.rows.iter().any(|r| r["Put"]["table"] == "review_fixes"));
    assert!(r.committed.is_empty());
}
#[test]
fn mandatory_optional_fields_and_failures() {
    for model in [None, Some("model".into()), Some("other".into())] {
        for head in [None, Some(1)] {
            let mut s = schedule("reviewer", json!({"verdict":"out_of_room"}));
            s.head = head;
            if let Observation::WorkerReply { model: m, .. } = &mut s.observation {
                *m = model.clone();
            }
            let r = routed(&s);
            assert!(r.result.to_string().contains("ReviewerOutOfRoom"));
        }
    }
    for verdict in ["agree", "disagree"] {
        let mut r = reply("audit");
        r["verdict"] = json!(verdict);
        r["objections"] = if verdict == "agree" {
            json!([])
        } else {
            json!(["A concrete objection"])
        };
        let mut s = schedule("audit", r);
        s.protected = vec![1];
        assert!(routed(&s).result.to_string().contains("ProtectedPaths"));
    }
    let mut s = schedule("planner", reply("planner"));
    s.protected = vec![1];
    assert!(routed(&s).result.to_string().contains("ProtectedPaths"));
    let mut s = schedule("ingest", reply("ingest"));
    s.prerequisites = vec![1];
    assert!(compare(&s).result.get("error").is_some());
    let mut s = schedule("planner", reply("planner"));
    s.snapshot.rows.retain(|r| r.table != "requests");
    assert!(compare(&s).result.get("error").is_some());
    let mut s = schedule("planner", reply("planner"));
    s.intern_failures = vec![false, true];
    let r = compare(&s);
    assert!(r.result.get("error").is_some());
    assert!(r.committed.is_empty());
    for read in [Read::Store, Read::Json, Read::Value(Some(json!({})))] {
        let mut s = schedule(
            "reviewer",
            json!({"verdict":"changes","blocking_findings":blockers(2,8),"findings":[]}),
        );
        s.documents = vec![read];
        assert!(compare(&s).result.get("error").is_some());
        let mut s = schedule("planner", reply("planner"));
        s.documents = s.documents.into_iter().map(|_| Read::Store).collect();
        assert!(compare(&s).result.get("error").is_some());
    }
}
#[test]
fn fixed_seed_reproduction_read_schedules() {
    let config = Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x7602b2027),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "crates/controller/tests/worker_translate_io/proptest-regressions.txt",
        ))),
        ..Config::default()
    };
    TestRunner::new(config)
        .run(
            &(
                any::<bool>(),
                prop::collection::vec(0u8..6, 2..6),
                prop::collection::vec(any::<bool>(), 0..4),
            ),
            |(tests, tape, failures)| {
                let mut s = reproduction(tests);
                s.intern_failures = failures;
                s.plans = tape
                    .iter()
                    .enumerate()
                    .map(|(i, v)| match v {
                        0 => Read::Store,
                        1 => Read::Json,
                        2 => Read::Value(None),
                        3 => Read::Value(Some(json!({}))),
                        _ => Read::Value(Some(plan(&format!("read-{i}")))),
                    })
                    .collect();
                compare(&s);
                Ok(())
            },
        )
        .unwrap();
}
#[test]
fn mandatory_all_reply_outcomes_and_plan_budget() {
    for outcome in ["duplicate", "already_done", "not_actionable"] {
        let s = schedule(
            "ingest",
            json!({"kind":"early_end","outcome":outcome,"reason":"Safe evidence"}),
        );
        assert!(routed(&s).result.to_string().contains("IngestEarlyEnd"));
    }
    assert!(
        routed(&schedule(
            "audit",
            json!({"verdict":"disagree","objections":["Concrete objection"]})
        ))
        .result
        .to_string()
        .contains("AuditDisagrees")
    );
    for (kind, expected) in [
        ("cant_meet", "MakerCantMeet"),
        ("out_of_room", "MakerOutOfRoom"),
        ("blocked", "RunFailed"),
    ] {
        let reply = match kind {
            "cant_meet" => json!({"kind":kind,"report":"Cannot meet the goal"}),
            "blocked" => {
                json!({"kind":kind,"report":"Environment failed","evidence":{"tool_error":"tool-error-1","tool":"read","class":"EACCES"}})
            }
            _ => json!({"kind":kind}),
        };
        assert!(
            routed(&schedule("maker", reply))
                .result
                .to_string()
                .contains(expected)
        );
    }
    let mut reply = reply("planner");
    reply["checks"][0]["node_ids"] = json!(["x".repeat(4 * 1024 * 1024)]);
    let r = routed(&schedule("planner", reply));
    assert!(r.result.to_string().contains("PlanCantMeet"));
    let plans = r
        .rows
        .iter()
        .find(|r| r["Put"]["table"] == "plans")
        .unwrap();
    assert!(
        !plans["Put"]["value"]["violations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for optional in [false, true] {
        let mut s = schedule(
            "planner",
            json!({"kind":"cant_meet","report":"No solution","files":["safe.rs"],"violations":["A violation","A violation"],"forecast":forecast()}),
        );
        s.documents = vec![Read::Value(Some(json!([
            "Already recorded",
            "A violation"
        ])))];
        if optional && let Observation::WorkerReply { reply, .. } = &mut s.observation {
            reply["memory_ids"] = json!(["safe-memory"]);
        }
        assert!(routed(&s).result.to_string().contains("PlanCantMeet"));
    }
}

#[test]
fn mandatory_invalid_maker_paths_and_unquoted_scope_citations() {
    let failed = json!({"event":Event::Reply {node:NodeId(0),generation:7,reply:Reply::RunFailed},"detailed_review":false});
    for path in ["../outside.rs", "/absolute.rs", "dir/../outside.rs", ".git/config", "dir\\file.rs"] {
        let s = schedule("maker", json!({"kind":"needs_files","files":[{"path":path,"why":"Need this file"}],"fits":true}));
        let r = compare(&s);
        assert_eq!(r.result, failed, "invalid maker path {path}");
        assert!(r.calls.is_empty(), "path validation precedes every I/O call");
        assert!(r.rows.is_empty());
        assert_eq!(r.writes, 0);
    }
    let s = schedule("reviewer", json!({"verdict":"changes","blocking_findings":[{"citation":{"kind":"standing_rule","id":"request-scope"},"problem":"Unrelated edit","evidence":"No changed-hunk quote"}],"findings":[]}));
    let r = compare(&s);
    assert_eq!(r.result, failed);
    assert_eq!(r.calls.iter().map(|call|call["op"].as_str().unwrap()).collect::<Vec<_>>(), ["row.push", "protected_reply", "worker_materials"]);
    assert_eq!(r.rows, vec![json!({"Delete":{"table":"model_quotas","id":"model"}})]);
    assert_eq!(r.calls[1]["args"]["role"], "reviewer");
    assert_eq!(r.calls[2]["args"]["args"], json!(["0"]));
    assert_eq!(r.writes, 0);
    assert!(r.rows.iter().all(|row|row["Put"]["table"]!="maker_findings"));
}

#[test]
fn fixed_seed_successful_findings_persistence_and_intern_crashes() {
    let config = Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x7602b2028),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "crates/controller/tests/worker_translate_io/proptest-regressions.txt",
        ))),
        ..Config::default()
    };
    let reached = std::cell::RefCell::new(BTreeSet::new());
    TestRunner::new(config).run(&(1usize..21,1usize..3501,any::<bool>(),prop::collection::vec(0u8..4,0..4)), |(count,size,first_plan,tail)| {
        for existing in [None, Some(json!([])), Some(json!(["Prior report"]))] {
            let mut s=schedule("reviewer",json!({"verdict":"changes","blocking_findings":blockers(count,size),"findings":[]}));
            // Valid first entries reach persistence. Later independently varying
            // entries detect accidental extra reads through the trace comparison.
            s.documents=vec![Read::Value(existing.clone())];
            s.materials=vec![Read::Value(Some(json!({"standing_rules":[{"id":format!("rule-{size}")}]})))];
            for item in &tail {
                let read=match item {0=>Read::Store,1=>Read::Json,2=>Read::Value(None),_=>Read::Value(Some(json!({"unexpected":size})))};
                s.documents.push(read.clone());s.materials.push(read);
            }
            let r=compare(&s);
            assert_eq!(r.result,json!({"event":Event::Reply{node:NodeId(0),generation:7,reply:Reply::ChangesRequested},"detailed_review":true}));
            assert_eq!(r.calls.iter().map(|c|c["op"].as_str().unwrap()).collect::<Vec<_>>(),["row.push","protected_reply","worker_materials","row.push","worker_document","row.push","row.push","row.push"]);
            let feedback=r.rows.iter().find(|r|r["Put"]["table"]=="maker_findings").unwrap()["Put"]["value"].clone();
            let reports:Vec<_>=feedback.as_array().unwrap().iter().filter(|f|f["kind"]=="review_finding").collect();
            assert!(!reports.is_empty());
            assert!(reports.iter().all(|f|f["reviewed_sha"]==OLD_HEAD));
            assert!(reports.iter().map(|f|f["report"].as_str().unwrap().chars().count()).sum::<usize>()<=findings::MAX_REPORT_CHARS);
            assert!(r.committed.iter().any(|row|row["Put"]["table"]=="maker_findings"));
            reached.borrow_mut().insert((count,size,existing.is_some(),tail.clone()));

            // Reviewer changes have no intern writes. Feed their actual persisted
            // findings into the next maker reply, where compare replays a crash
            // before and after the commit intern INSERT.
            let mut maker=schedule("maker",reply("maker"));maker.round=1;
            maker.documents=vec![Read::Value(Some(feedback.clone()))];
            maker.plans=vec![Read::Value(first_plan.then(||plan("first"))),Read::Value(Some(plan("second")))];
            if let Observation::WorkerReply{reply,..}=&mut maker.observation {
                reply["reproductions"]=json!(reports.iter().map(|f|json!({"finding_id":f["id"],"reason":"Documented review finding"})).collect::<Vec<_>>());
            }
            let made=compare(&maker);
            assert!(made.result.to_string().contains("MakerDone"),"{}",made.result);
            assert_eq!(made.writes,1);
            assert!(made.rows.iter().any(|row|row["Put"]["table"]=="review_fixes"));
        }
        Ok(())
    }).unwrap();
    assert!(reached.borrow().len()>=2,"successful populated rows must span different findings/read tapes");
}
