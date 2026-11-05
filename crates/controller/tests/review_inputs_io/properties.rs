use super::*;
use io::{Read, Schedule, compare};
use proptest::{prelude::*, test_runner::{Config, RngSeed, FileFailurePersistence}};
const BASE: &str = "1111111111111111111111111111111111111111";
const HEAD: &str = "2222222222222222222222222222222222222222";
const REVIEWED: &str = "3333333333333333333333333333333333333333";
fn fixture() -> Schedule {
    let delivery: Delivery = serde_json::from_value(json!({"key":"review:0", "epoch":1,
        "output":{"Effect":{"node":0,"seq":1,"kind":{"StartRun":"Review"},"head":null}},"generation":1,
        "head_sha":HEAD,"repository":{"owner":"o","name":"r","installation":1},"issue":1,"paths":[],"attempt":1,"round":0,
        "pull_request":null,"checks":{"repository":"recorded-repository","base_sha":BASE,"plan":[]}})).unwrap();
    let snapshot = tl3_store::Snapshot {rows: vec![
        tl3_store::Row {table:"goals".into(),id:"0".into(),value:json!([{"description":"Do work","verification":"test"}])},
        tl3_store::Row {table:"check_reports".into(),id:format!("0:{HEAD}"),value:json!({"base_sha":BASE,"results":[]})}
    ]};
    Schedule {delivery,snapshot,session:None,tapes:BTreeMap::from([
        ("open".into(),vec![Read::Value(Value::Null)]),
        ("merge_base".into(),vec![Read::Value(json!(BASE));3]),
        ("diff".into(),vec![Read::Value(json!(b"diff".to_vec()));2]),
        ("worker_materials".into(),vec![Read::Value(json!({}));2]),
        ("worker_document".into(),vec![Read::Value(Value::Null);3]),
        ("changed_paths".into(),vec![Read::Value(json!([]))]),
        ("tree".into(),vec![Read::Value(json!(HEAD)),Read::Value(json!(BASE)),Read::Value(json!(HEAD))]),
    ])}
}
fn value(r: &io::Run) -> Value { serde_json::from_slice(r.result.as_ref().unwrap()).unwrap() }
fn ops(r: &io::Run) -> Vec<&str> { r.trace.iter().map(|c| c["op"].as_str().unwrap()).collect() }
#[test]
fn frozen_and_current_sources() {
    use sha2::{Digest,Sha256};
    let base=include_str!("base-review.rs.txt");
    let pins:Value=serde_json::from_str(include_str!("pins.json")).unwrap();
    assert_eq!(format!("{:x}",Sha256::digest(base)),pins["sha256"].as_str().unwrap());
    let part=|s:&str| s[s.find("pub(super) fn inputs(").unwrap()..s.find("pub(crate) fn criteria(").unwrap()].to_owned();
    assert_eq!(include_str!("reference.rs"),format!("use super::*;\n{}",part(base)));
    let production=std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),"/src/adapters/workers/review.rs")).unwrap();
    let current=include_str!("current.rs");
    // Red step explicitly stubs only the current side. Green removes this exception.
    if !current.contains("H17 extraction stub") { assert_eq!(current,format!("use super::*;\n{}",part(&production))); }
    assert!(base.contains(include_str!("criteria.rs").strip_prefix("use super::*;\n").unwrap()));
    assert!(production.contains(include_str!("criteria.rs").strip_prefix("use super::*;\n").unwrap()));
}
#[test]
fn mandatory_empty_exact_wire_and_trace() {
    let s=fixture(); let r=compare(&s); let v=value(&r);
    assert_eq!(v,json!({"goals":[{"id":"goal-1","description":"Do work","verification":"test"}],"base_sha":BASE,"check_base_sha":BASE,"head_sha":HEAD,"diff":"diff","checks":[],"standing_rules":v["standing_rules"]}));
    assert_eq!(v["standing_rules"].as_array().unwrap().len(),2);
    assert_eq!(ops(&r),["open","merge_base","worker_materials","diff","worker_document","worker_document","worker_document","validate_inputs"]);
    assert_eq!(r.trace[7]["args"],json!(["reviewer",v]));
    assert_eq!(r.trace[4]["args"],json!(["0","review_fixes"]));
    assert_eq!(r.trace[5]["args"],json!(["0","maker_replies"]));
    assert_eq!(r.trace[6]["args"],json!(["0","maker_replies"]));
}
proptest! {
    #![proptest_config(Config {cases:64,rng_seed:RngSeed::Fixed(0x17_0730_2026),failure_persistence:Some(Box::new(FileFailurePersistence::Direct("/tmp/review-inputs-regressions.txt"))),..Config::default()})]
    #[test]
    fn generated_read_tapes(flags in any::<u16>(), fail in 0usize..24, classes in prop::collection::vec(0u8..5,0..20)) {
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
            if class>0 {tape[at]=match class {1=>Read::Io,2=>Read::Json,3=>Read::Git,_=>Read::Invalid};}
        }
        compare(&s);
    }
}
