use super::*;
use io::{NEW_HEAD, OLD_HEAD, Schedule, compare, incident, plan, row, stable};
use proptest::{prelude::*, test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner}};

fn required() -> adapters::mergify::RequiredCheck {
    adapters::mergify::RequiredCheck { context:"CI".into(),app:Some(1) }
}
fn evidence(tag: u8, head: &str, flags: u8) -> Evidence {
    match tag%11 {
        0=>Evidence::ChecksUnavailable {permissions:vec!["statuses:403".into(),"checks:403".into(),"statuses:403".into()]},
        1=>Evidence::CheckMissing {head:head.into(),required:required()},
        2=>Evidence::QueueBlocked {head:head.into(),conditions:vec!["label=approved".into()],failed:vec!["optional CI".into()]},
        3=>Evidence::CheckReported {head:head.into(),required:required()},
        4=>Evidence::Merged {head:head.into(),checks_passed:flags&1!=0},
        5=>Evidence::CiFailed {check:42,completed_at:Some("revision".into()),head:head.into(),infra:flags&1!=0,head_check:flags&2!=0,
            details:findings::QueueCiEvidence {checks:vec![],failed_tests:vec![findings::FailedTest {test:"tests/outside.py::case".into(),message:"failed".into()}],
                changed_paths:Some(vec!["src/changed.rs".into()]),complete:true,decision:None}},
        6=>Evidence::Conflict {check:42,completed_at:None,head:head.into()},
        7=>Evidence::DequeuedByCommand {check:42,completed_at:None,head:head.into()},
        8=>Evidence::DequeuedByPerson {event:42},
        9=>Evidence::ClosedByPerson {event:42},
        _=>Evidence::PushByOther {head:head.into()},
    }
}
fn event(run: &io::Run) -> &Value { &run.result["event"]["Fact"]["fact"] }
fn has_table(run: &io::Run, table: &str) -> bool { run.rows.iter().any(|r|r["table"]==table) }
fn prior(s: &mut Schedule, mask: u8) {
    let mut value=json!({"repository":s.fact.repository,"number":s.fact.number,
        "evidence":{"CiFailed":{"head":OLD_HEAD,"infra":true,"details":{"failed_tests":[{"test":"tests/outside.py::case"}]}}}});
    if mask&1!=0 {value["repository"]["name"]=json!("other");}
    if mask&2!=0 {value["number"]=json!(101);}
    if mask&4!=0 {value["evidence"]["CiFailed"]["head"]=json!(NEW_HEAD);}
    if mask&8!=0 {value["evidence"]["CiFailed"]["infra"]=json!(false);}
    if mask&16!=0 {value["evidence"]["CiFailed"]["details"]["failed_tests"]=json!([{"test":"tests/different.py::case"}]);}
    if mask&32!=0 {value["evidence"]["CiFailed"]["details"]["failed_tests"]=json!({"not":"an array"});}
    if mask&64!=0 {value["evidence"]["CiFailed"]["details"]["failed_tests"]=json!([null,{"test":42},{"test":"tests/outside.py::case"}]);}
    s.snapshot.rows.push(row(if mask&128!=0 {"other"}else{"queue_facts"},"prior",value));
}
fn add_incidents(s: &mut Schedule, open: bool) {
    let ids=match &s.fact.evidence {
        Evidence::ChecksUnavailable {permissions}=>permissions.iter().map(|p|looks::check_setup_incident_id(NodeId(0),p)).collect(),
        Evidence::CheckMissing {head,required}|Evidence::CheckReported {head,required}=>vec![looks::missing_check_incident_id(NodeId(0),head,required)],
        Evidence::QueueBlocked {head,conditions,..}=>vec![looks::queue_blocked_incident_id(NodeId(0),head,conditions)],
        _=>vec![],
    };
    for id in ids {s.snapshot.rows.push(row("incidents",&id,incident(&id,open,"existing")));}
}
#[test]
fn mandatory_closed_and_stale_head() {
    for tag in 0..11 {
        let mut s=stable(evidence(tag,NEW_HEAD,0)); s.phase=2;
        let run=compare(&s);
        if tag==4 {assert!(event(&run).get("PrMerged").is_some());} else {assert!(run.rows.is_empty());assert!(run.result["event"].is_null());}
    }
    let mut s=stable(evidence(4,NEW_HEAD,1));s.phase=2;s.merged=true;
    assert!(compare(&s).rows.is_empty());
    for tag in [5,6,7] {
        let run=compare(&stable(evidence(tag,NEW_HEAD,0)));
        assert!(run.result["event"].is_null());assert!(has_table(&run,"queue_facts"));assert_eq!(run.writes,1);
    }
}
#[test]
fn mandatory_setup_incidents_and_dedup() {
    for tag in [0,1,2] {
        let mut s=stable(evidence(tag,OLD_HEAD,0));
        let run=compare(&s);
        assert!(has_table(&run,"incidents"));assert!(has_table(&run,"queue_facts"));
        assert!(run.result["event"].is_null());assert_eq!(run.writes,0);assert!(!run.effects.is_empty());
        add_incidents(&mut s,true);
        assert!(compare(&s).rows.is_empty());
        let mut cleared=stable(evidence(tag,OLD_HEAD,0));add_incidents(&mut cleared,false);
        let run=compare(&cleared);
        if tag==0 {assert!(has_table(&run,"incidents"));} else {assert!(run.rows.is_empty());}
    }
    for tag in [1,2] {let s=stable(evidence(tag,NEW_HEAD,0));assert!(compare(&s).rows.is_empty());}
    let mut queued=stable(evidence(2,OLD_HEAD,0));queued.phase=1;queued.enqueue=false;
    assert!(has_table(&compare(&queued),"incidents"));
}
#[test]
fn mandatory_check_reported_clears_only_own_open_incident() {
    let mut s=stable(evidence(3,OLD_HEAD,0));add_incidents(&mut s,true);
    s.snapshot.rows.push(row("incidents","unrelated",incident("unrelated",true,"leave open")));
    s.phase=3;
    let run=compare(&s);
    assert_eq!(run.rows.len(),1);assert_eq!(run.rows[0]["value"]["open"],false);
    assert_ne!(run.rows[0]["id"],"unrelated");assert!(run.result["event"].is_null());assert_eq!(run.writes,0);
    let mut absent=stable(evidence(3,OLD_HEAD,0));assert!(compare(&absent).rows.is_empty());
    add_incidents(&mut absent,false);assert!(compare(&absent).rows.is_empty());
}
#[test]
fn mandatory_repeated_outside_and_upstream_ci() {
    let mut s=stable(evidence(5,OLD_HEAD,0));s.plans=vec![Ok(Some(plan("outside")))];
    assert_eq!(event(&compare(&s))["CiFailed"]["infra"],true);
    prior(&mut s,0);
    let run=compare(&s);assert_eq!(event(&run)["CiFailed"]["infra"],false);
    assert_eq!(run.rows.last().unwrap()["value"]["evidence"]["CiFailed"]["details"]["decision"],"repeated_test");
    for mask in [1,2,4,8,16,32,128] {let mut s=stable(evidence(5,OLD_HEAD,0));s.plans=vec![Ok(Some(plan("outside")))];prior(&mut s,mask);assert_eq!(event(&compare(&s))["CiFailed"]["infra"],true);}
    for infra in [false,true] {let s=stable(evidence(5,OLD_HEAD,u8::from(infra)));let run=compare(&s);
        assert_eq!(event(&run)["CiFailed"]["infra"],infra);
        assert_eq!(run.rows.last().unwrap()["value"]["evidence"]["CiFailed"]["details"]["decision"],"upstream_classification");}
}
#[test]
fn mandatory_push_previous_sha_and_plan_carry_forward() {
    let mut s=stable(evidence(10,NEW_HEAD,0));s.plans=vec![Ok(Some(plan("first"))),Ok(Some(plan("second")))];
    let run=compare(&s);
    assert_eq!(run.calls[0]["op"],"check_run");assert_eq!(run.calls[0]["args"]["sha"],OLD_HEAD);
    assert_eq!(run.rows[0]["table"],"check_plans");assert_eq!(run.rows[0]["id"],format!("0:{NEW_HEAD}"));
    assert_eq!(run.rows[0]["value"],json!(plan("first")));assert!(event(&run).get("PushByOther").is_some());assert_eq!(run.writes,1);
    assert_eq!(run.calls.iter().filter(|c|c["op"]=="check_run").count(),1);
    assert!(run.persisted_intern.contains_key(&("0".into(),NEW_HEAD.into())));
    s.current=None;s.plans=vec![Ok(None),Ok(Some(plan("must not be sampled")))];
    let run=compare(&s);assert_eq!(run.calls[0]["args"]["sha"],Value::Null);assert!(!has_table(&run,"check_plans"));
}
#[test]
fn mandatory_head_check_admission_and_all_kernel_routes() {
    for phase in 0..4 {for enqueue in [false,true] {for cancelling in [false,true] {
        let mut s=stable(evidence(5,OLD_HEAD,2));s.phase=phase;s.enqueue=enqueue;s.cancelling=cancelling;
        let run=compare(&s);let admitted=phase==1 || phase==0&&enqueue&&!cancelling;
        assert_eq!(!run.result["event"].is_null(),admitted);
        assert!(!run.calls.iter().any(|c|c["op"]=="check_run"));
    }}}
    for tag in 4..11 {let run=compare(&stable(evidence(tag,OLD_HEAD,0)));assert!(!run.result["event"].is_null());assert!(has_table(&run,"queue_facts"));}
}
#[test]
fn queue_io_schedules() {
    let mut runner=TestRunner::new(Config {cases:256,max_shrink_iters:2048,rng_seed:RngSeed::Fixed(0x0047_1760_2026),
        failure_persistence:Some(Box::new(FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"),"/tests/queue_translate_io/proptest-regressions.txt")))),..Config::default()});
    let strategy=(any::<u8>(),any::<u8>(),any::<u8>(),prop::collection::vec(any::<u8>(),0..32),prop::collection::vec(0u8..5,0..8),prop::collection::vec(any::<bool>(),0..3));
    let comparisons=std::cell::Cell::new(0);
    runner.run(&strategy,|(tag,flags,shape,history,reads,failures)| {
        let mut s=stable(evidence(tag,if flags&4!=0 {NEW_HEAD}else{OLD_HEAD},flags));
        s.phase=(shape>>4)%4;s.enqueue=shape&1!=0;s.cancelling=shape&2!=0;s.merged=shape&4!=0;
        s.current=if shape&8!=0 {None}else{Some(1)};s.approved=if shape&64!=0 {None}else{Some(1)};
        if shape&128!=0 {add_incidents(&mut s,flags&8!=0);}
        for mask in history {prior(&mut s,mask);}
        s.plans=reads.into_iter().enumerate().map(|(n,r)|match r {0=>Ok(None),1=>Err("changing check_run error"),_=>Ok(Some(plan(&format!("sample-{n}-{r}"))))}).collect();
        s.intern_failures=failures;
        let run=compare(&s);comparisons.set(comparisons.get()+1+run.writes*2);Ok(())
    }).unwrap();
    eprintln!("Queue: 256 schedules, {} complete/crash comparisons, seed 0x004717602026",comparisons.get());
}
fn arm(source: &str) -> &str {
    let start=source.find("            Observation::Queue(mut fact) => {").unwrap();
    let end=source[start..].find("            Observation::GitHubComment {").map(|at|start+at)
        .unwrap_or_else(|| source[start..].find("        };\n").unwrap()+start);
    &source[start..end]
}
#[test]
fn pinned_reference_and_current_source() {
    assert_eq!(format!("{:x}",Sha256::digest(include_bytes!("base-queue.rs.txt"))),"2812c0b500274973e4c7367c18d37c0d39517dfaa202c80f3b59d751b6a5601c");
    assert_eq!(arm(include_str!("reference.rs")),include_str!("base-queue.rs.txt"));
    assert_eq!(arm(include_str!("current.rs")),arm(include_str!("../../src/lib.rs")));
    for (bytes,expected) in [(include_bytes!("base-adapter.rs.txt").as_slice(),"4c23cc4a17eb17793dfe1cfc4b911de49660267bbebb306269e8bc673eb255f9"),
        (include_bytes!("base-looks.rs.txt").as_slice(),"2ad7e468cb0a8d3d22cfe5511b0aa6193cb8236dbca7f8fd7f10a7784b315bee"),
        (include_bytes!("base-ci.rs.txt").as_slice(),"6c5536439762f6d7a7db7db03bbee0945a9e2829691e16c1e209519e6f44c016")] {
        assert_eq!(format!("{:x}",Sha256::digest(bytes)),expected);
    }
}
