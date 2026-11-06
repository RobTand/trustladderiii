//! One executable render/validate/refuse contract per role. Semantic rows belong
//! to C3/C4; this case exercises the shared interface with a complete reply edit.
use serde_json::{Value, json};
use tl3_contracts::{role_contract::*, rules::*};

fn exercise<C: RoleContract<Facts = Value>>(contract: C, reply: Value) {
    assert!(C::validate_input(Value::Null).is_err());
    let parsed = C::validate_reply(reply.clone()).unwrap_or_else(|e| panic!("{}: {e}", C::ROLE.as_str()));
    assert_eq!(serde_json::to_value(parsed).unwrap(), reply);
    assert!(C::validate_reply(json!({"unknown":true})).is_err());
    let rendered = contract.render(RenderOptions { test_cpus: 7, generation: false }).unwrap();
    assert_eq!(rendered.guidance, guidance::ONLY_JSON);
    assert_eq!(rendered.model_schema["$id"], C::reply_schema()["$id"]);
    assert!(rendered.model_schema.is_object());
    let policy = Policy { terminals: &[], source_editors: &[], schema_membership: None };
    let before = json!({"invalid":"candidate"});
    let rows = contract.evaluate(&before, &policy).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].role, C::ROLE);
    assert!(render_all(&rows, C::ROLE, RenderContext::Reply, &policy).unwrap().contains("Permitted edit: replace reply field \"\""));
    assert!(render_all(&rows, C::ROLE, RenderContext::Tool, &policy).is_err());
    assert!(permits(&rows, &before, &reply, C::ROLE, &policy).unwrap());
    assert!(!permits(&rows, &before, &json!({"unrelated":1}), C::ROLE, &policy).unwrap());
    let wire = serde_json::to_value(&rows).unwrap();
    assert_eq!(wire[0]["role"], C::ROLE.as_str());
    assert_eq!(serde_json::from_value::<Vec<Violation>>(wire).unwrap(), rows);
}
fn rule(role: Role, reply: Value) -> Rule<Value> {
    Rule { id: "contract.reply", check: |value| vec![value.clone()], names: |_| vec![Name { kind: NameKind::Field, value: String::new() }], bad_value: Clone::clone, allowed: Callback::Static(Allowed::Schema { keyword: SchemaKeyword::Const, value: reply.clone() }), repair: Callback::Static(Repair::Edit { pointer: String::new().try_into().unwrap(), op: EditOp::Replace, allowed: Allowed::Schema { keyword: SchemaKeyword::Const, value: reply } }), gating: false, roles: vec![role] }
}
macro_rules! case { ($name:ident,$contract:ident,$role:ident,$reply:expr) => { #[test] fn $name() { let reply=$reply;exercise($contract::<Value> { rules: vec![rule(Role::$role,reply.clone())] },reply); } }; }
case!(ingest,Ingest,Ingest,json!({"kind":"early_end","outcome":"not_actionable","reason":"Need a request"}));
case!(audit,Audit,Audit,json!({"verdict":"agree","objections":[]}));
case!(planner,Planner,Planner,json!({"kind":"leaf","files":[],"checks":[],"no_design_questions":true,"lands_on_its_own":true}));
case!(maker,Maker,Maker,json!({"kind":"cant_meet","report":"Need a declared path"}));
case!(reviewer,Reviewer,Reviewer,json!({"verdict":"out_of_room"}));
case!(classifier,Classifier,Classifier,json!({"kind":"stop"}));
case!(writer,Writer,Writer,json!({"text":"Completed"}));
case!(postmortem,Postmortem,Postmortem,json!({"postmortem":{"summary":"Done","cause":"outside","went_wrong":"","timeline":[],"process_failures":[],"recommendations":[],"notes":"","lessons":[]},"grades":[],"assessment_ids":[],"usage":{"traces":0,"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"models":{"postmortem":"model","grades":"model"}}));
