use super::*;
use io::{compare, schedule, Read, Schedule, OLD_HEAD, NEW_HEAD};
use proptest::{prelude::*, test_runner::{TestRunner, Config, RngSeed, FileFailurePersistence}};
use sha2::{Digest, Sha256};
fn arm(source:&str)->&str {
    let a=source.find("            Observation::WorkerFailed {").unwrap();
    let b=source[a..].find("            Observation::Plan {").map_or(source.len(),|b|a+b);
    &source[a..b]
}
#[test]
fn production_and_reference_sources_are_pinned() {
    let pins:Value=serde_json::from_str(include_str!("pins.json")).unwrap();
    let base=include_str!("base-arms.rs.txt");
    assert_eq!(format!("{:x}",Sha256::digest(base)),pins["blobs"]["arms"]["sha256"]);
    let source=include_str!("../../src/lib.rs");
    for (copy, expected) in [(include_str!("reference.rs"),base),(include_str!("current.rs"),arm(source))] {
        let a=copy.find("            Observation::WorkerFailed {").unwrap();
        let b=copy[a..].find("        };\n        *detail").unwrap()+a;
        assert_eq!(&copy[a..b],expected);
    }
    // Frozen helper blobs are separate from the two independently compiled arms.
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name,pin) in pins["blobs"].as_object().unwrap() {
        if name=="arms" {continue;}
        let frozen=std::fs::read(root.join(format!("crates/controller/tests/worker_translate_io/base-{name}.rs.txt"))).unwrap();
        assert_eq!(format!("{:x}",Sha256::digest(&frozen)),pin["sha256"]);
        let production=std::fs::read_to_string(root.join(pin["path"].as_str().unwrap())).unwrap();
        let compiled=std::fs::read_to_string(root.join(format!("crates/controller/tests/worker_translate_io/{name}.rs"))).unwrap();
        for part in pin["parts"].as_array().unwrap() {
            let part=part.as_str().unwrap();
            assert!(production.contains(part),"production leaf drift: {name}");
            let normalized=compiled.replace("pub(super) fn", "fn");
            assert!(compiled.contains(part)||normalized.contains(part),"compiled leaf drift: {name}");
        }
    }
}
fn forecast()->Value {json!({"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"safe.rs":0},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true})}
fn reply(role:&str)->Value {match role {
    "ingest"=>json!({"kind":"brief","brief":"Do work","goals":[{"description":"Do work","verification":"test"}],"memory_ids":[],"files":["safe.rs"],"forecast":forecast()}),
    "audit"=>json!({"verdict":"agree","objections":[]}),
    "planner"=>json!({"kind":"leaf","files":["safe.rs"],"checks":[{"kind":"regression","node_ids":["test_safe.py::test_safe"],"goal_ids":[0]}],"no_design_questions":true,"lands_on_its_own":true,"violations":[],"forecast":forecast()}),
    "maker"=>json!({"kind":"done","head":NEW_HEAD,"self_check":[{"goal_id":"goal-1","status":"met","evidence":"passed"}]}),
    _=>json!({"verdict":"approve","head_sha":OLD_HEAD,"blocking_findings":[],"findings":[]}),
}}
fn routed(s:&Schedule)->io::Run {let r=compare(s); assert!(r.result.get("event").is_some(),"{}",r.result);r}
#[test]
fn mandatory_roles_and_staleness() {
    for role in ["ingest","audit","planner","maker","reviewer"] {
        let s=schedule(role,reply(role));let r=routed(&s);
        assert!(!r.result.to_string().contains("RunFailed"),"{role}: {}",r.result);
        for stale in 0..3 {
            let mut s=s.clone();match stale {0=>s.run=None,1=>s.run=Some(6),_=>s.generation=6};
            let r=compare(&s);assert!(r.calls.is_empty());assert_eq!(r.result["event"],Value::Null);
        }
    }
    for role in ["writer","classifier","postmortem","unknown"] {
        let r=routed(&schedule(role,reply("audit")));assert!(r.result.to_string().contains("RunFailed"));assert!(r.calls.is_empty());
    }
}
#[test]
fn mandatory_role_validation_and_error_classes() {
    for role in ["ingest","audit","planner","maker","reviewer"] {
        let mut s=schedule(role,reply(role));s.phase=tl3_kernel::Phase::Queued;
        assert!(routed(&s).result.to_string().contains("RunFailed"));
        assert!(routed(&schedule(role,json!({}))).result.to_string().contains("RunFailed"));
        let mut s=schedule(role,reply(role));s.missing=true;
        assert!(compare(&s).result.to_string().contains("worker node missing"));
    }
    for read in [Read::Store,Read::Json,Read::Value(None),Read::Value(Some(json!({}))),Read::Value(Some(json!({"standing_rules":false})))] {
        let mut s=schedule("reviewer",reply("reviewer"));s.materials=vec![read];assert!(compare(&s).result.get("error").is_some());
    }
    for read in [Read::Store,Read::Json,Read::Value(None),Read::Value(Some(json!({})))] {
        let mut s=schedule("maker",reply("maker"));s.materials=vec![read];assert!(compare(&s).result.get("error").is_some());
    }
    for role in ["ingest","audit","planner"] {let mut s=schedule(role,reply(role));s.protected=vec![2];assert!(compare(&s).result.get("error").is_some());}
}
fn blockers(count:usize,size:usize)->Value {
    json!((0..count).map(|i|json!({"citation":{"kind":if i%2==0 {"goal"} else {"standing_rule"},"id":if i%2==0 {"goal-1"}else{"test-report-integrity"}},"problem":"é".repeat(size),"evidence":"Proof"})).collect::<Vec<_>>())
}
#[test]
fn mandatory_findings_budget_and_validation() {
    let mut s=schedule("reviewer",json!({"verdict":"changes","blocking_findings":blockers(20,3000),"findings":[]}));
    let r=routed(&s);assert_eq!(r.result["detailed_review"],true);
    let row=r.rows.iter().find(|r|r["Put"]["table"]=="maker_findings").unwrap();
    let items=row["Put"]["value"].as_array().unwrap();assert!(!items.is_empty()&&items.len()<20);
    assert!(items.iter().map(|i|i["report"].as_str().unwrap().chars().count()).sum::<usize>()<=16000);
    if let Observation::WorkerReply{reply,..}=&mut s.observation {reply["blocking_findings"][0]["citation"]["id"]=json!("unknown");}
    assert!(routed(&s).result.to_string().contains("RunFailed"));
    let mut s=schedule("reviewer",reply("reviewer"));
    if let Observation::WorkerReply{reply,..}=&mut s.observation{reply["head_sha"]=json!(NEW_HEAD);}
    assert!(routed(&s).result.to_string().contains("RunFailed"));
}
#[test]
fn mandatory_maker_self_check_and_intern_crashes() {
    let valid=schedule("maker",reply("maker"));assert!(routed(&valid).writes>0);
    for kind in 0..5 {
        let mut s=valid.clone();
        if let Observation::WorkerReply{reply,..}=&mut s.observation {match kind {
            0=>reply["self_check"][0]["goal_id"]=json!("unknown"),
            1=>{let check=reply["self_check"][0].clone();reply["self_check"].as_array_mut().unwrap().push(check);},
            2=>{reply["self_check"][0]=json!({"goal_id":"goal-1","status":"pending","why":"owned elsewhere"});},
            3=>{reply["self_check"][0]=json!({"goal_id":"goal-1","status":"not_met","why":"not met"});},
            _=>reply["reproductions"]=json!([{"finding_id":"missing","reason":"not applicable"}]),
        }}
        let r=routed(&s);assert!(r.result.to_string().contains(if kind==3 {"MakerCantMeet"}else{"RunFailed"}),"{}",r.result);
    }
    for failure in [false,true]{let mut s=valid.clone();s.intern_failures=vec![failure];compare(&s);}
    let mut s=schedule("maker",json!({"kind":"needs_files","files":[{"path":"one.rs","reason":"needed"},{"path":"two.rs","reason":"needed"}],"fits":true}));
    let r=routed(&s);assert_eq!(r.writes,3);assert!(r.result.to_string().contains("MakerNeedsFiles"));
    s.intern_failures=vec![false,true];assert!(compare(&s).result.get("error").is_some());
}
#[test]
fn mandatory_worker_failed_classes() {
    for class in ["infrastructure","validation","transport","quota","timeout","blocked","unknown",""] {
        let mut s=schedule("audit",Value::Null);s.observation=Observation::WorkerFailed{node:NodeId(0),generation:7,failure_class:class.into(),failure_message:format!("safe {class}")};
        let r=routed(&s);assert!(r.result.to_string().contains("RunFailed"));assert_eq!(r.rows.len(),1);
        s.run=None;assert!(compare(&s).calls.is_empty());s.run=Some(7);s.generation=6;assert!(compare(&s).calls.is_empty());
    }
}
#[test]
fn fixed_seed_changing_reads_and_optional_fields() {
    let config=Config{cases:256,rng_seed:RngSeed::Fixed(0x7602b2026),failure_persistence:Some(Box::new(FileFailurePersistence::Direct("crates/controller/tests/worker_translate_io/proptest-regressions.txt"))),..Config::default()};
    TestRunner::new(config).run(&(0usize..5,0u8..16,prop::collection::vec(0u8..8,0..12),prop::collection::vec(any::<bool>(),0..8)),|(role,bits,tape,failures)|{
        let role=["ingest","audit","planner","maker","reviewer"][role];
        let mut s=schedule(role,reply(role));s.intern_failures=failures;
        s.documents=tape.iter().map(|v|match v {0=>Read::Store,1=>Read::Json,2=>Read::Value(Some(json!({}))),3=>Read::Value(Some(json!([{"kind":"review_finding","id":"f","reviewed_sha":OLD_HEAD,"report":"fix","goal_ids":["goal-1"]}]))),_=>Read::Value(None)}).collect();
        if bits&1!=0 {s.head=None;}if bits&2!=0{s.round=1;}if bits&4!=0{s.run=None;}
        if let Observation::WorkerReply{reply,model,..}=&mut s.observation {
            if bits&8!=0 {*model=None;}
            if role=="reviewer"&&bits&1!=0 {*reply=json!({"verdict":"changes","blocking_findings":blockers(1+tape.len(),1+usize::from(bits)*200),"findings":[]});}
        }
        compare(&s);Ok(())
    }).unwrap();
}
