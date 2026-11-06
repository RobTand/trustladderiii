//! One executable render/validate/refuse contract per role. Semantic rows belong
//! to C3/C4; this case exercises the shared interface with a complete reply edit.
use serde_json::{Value, json};
use tl3_contracts::{role_contract::*, rules::*};

fn exercise<C: RoleContract<Facts = Value>>(contract: C, reply: Value) {
    assert!(C::validate_input(Value::Null).is_err());
    C::validate_input(input(C::ROLE)).unwrap_or_else(|e| panic!("{} input: {e}", C::ROLE.as_str()));
    let parsed =
        C::validate_reply(reply.clone()).unwrap_or_else(|e| panic!("{}: {e}", C::ROLE.as_str()));
    assert_eq!(serde_json::to_value(parsed).unwrap(), reply);
    assert!(C::validate_reply(json!({"unknown":true})).is_err());
    let rendered = contract
        .render(RenderOptions {
            test_cpus: 7,
            generation: false,
        })
        .unwrap();
    assert_eq!(rendered.guidance, guidance::ONLY_JSON);
    assert_eq!(rendered.model_schema["$id"], C::reply_schema()["$id"]);
    assert!(rendered.model_schema.is_object());
    let policy = Policy {
        terminals: &[],
        source_editors: &[],
        schema_membership: None,
    };
    let before = json!({"invalid":"candidate"});
    let rows = contract.evaluate(&before, &policy).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].role, C::ROLE);
    assert!(
        render_all(&rows, C::ROLE, RenderContext::Reply, &policy)
            .unwrap()
            .contains("Permitted edit: replace reply field \"\"")
    );
    assert!(render_all(&rows, C::ROLE, RenderContext::Tool, &policy).is_err());
    assert!(permits(&rows, &before, &reply, C::ROLE, &policy).unwrap());
    assert!(!permits(&rows, &before, &json!({"unrelated":1}), C::ROLE, &policy).unwrap());
    let wire = serde_json::to_value(&rows).unwrap();
    assert_eq!(wire[0]["role"], C::ROLE.as_str());
    assert_eq!(
        serde_json::from_value::<Vec<Violation>>(wire).unwrap(),
        rows
    );
}
fn rule(role: Role, reply: Value) -> Rule<Value> {
    Rule {
        id: "contract.reply",
        check: |value| vec![value.clone()],
        names: |_| {
            vec![Name {
                kind: NameKind::Field,
                value: String::new(),
            }]
        },
        bad_value: Clone::clone,
        allowed: Callback::Static(Allowed::Schema {
            keyword: SchemaKeyword::Const,
            value: reply.clone(),
        }),
        repair: Callback::Static(Repair::Edit {
            pointer: String::new().try_into().unwrap(),
            op: EditOp::Replace,
            allowed: Allowed::Schema {
                keyword: SchemaKeyword::Const,
                value: reply,
            },
        }),
        gating: false,
        roles: vec![role],
    }
}
macro_rules! case {
    ($name:ident,$contract:ident,$role:ident,$reply:expr) => {
        #[test]
        fn $name() {
            let reply = $reply;
            exercise(
                $contract::<Value> {
                    rules: vec![rule(Role::$role, reply.clone())],
                },
                reply,
            );
        }
    };
}
case!(
    ingest,
    Ingest,
    Ingest,
    json!({"kind":"early_end","outcome":"not_actionable","reason":"Need a request"})
);
case!(
    audit,
    Audit,
    Audit,
    json!({"verdict":"agree","objections":[]})
);
case!(
    planner,
    Planner,
    Planner,
    json!({"kind":"leaf","files":[],"checks":[],"no_design_questions":true,"lands_on_its_own":true})
);
case!(
    maker,
    Maker,
    Maker,
    json!({"kind":"cant_meet","report":"Need a declared path"})
);
case!(
    reviewer,
    Reviewer,
    Reviewer,
    json!({"verdict":"out_of_room"})
);
case!(classifier, Classifier, Classifier, json!({"kind":"stop"}));
case!(writer, Writer, Writer, json!({"text":"Completed"}));
case!(
    postmortem,
    Postmortem,
    Postmortem,
    json!({"postmortem":{"summary":"Done","cause":"outside","went_wrong":"","timeline":[],"process_failures":[],"recommendations":[],"notes":"","lessons":[]},"grades":[],"assessment_ids":[],"usage":{"traces":0,"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"models":{"postmortem":"model","grades":"model"}})
);

fn input(role: Role) -> Value {
    let issue =
        json!({"number":1,"title":"Fix","body":"A request","author":"requester","comments":[]});
    let repo = json!({"readme":"","file_list":[]});
    let brief = json!({"kind":"brief","brief":"Fix","goals":[{"description":"Correct behavior","verification":"test"}],"memory_ids":[]});
    match role {
        Role::Ingest => json!({"issue":issue,"repo":repo}),
        Role::Audit => json!({"issue":issue,"repo":repo,"ingest":brief}),
        Role::Planner => {
            json!({"must_split":false,"findings":[],"protected_paths":[],"ingest":brief,"forecast":{"fit_tokens":100,"context_tokens":1000,"overhead_tokens":{"prompt":1}}})
        }
        Role::Maker => {
            json!({"brief":"Fix","goals":[{"id":"g0","description":"Correct behavior"}],"plan":{"files":["src/lib.rs"],"checks":[{"name":"test","kind":"regression","tests":["tests/test_api.py::test_status"],"goal_ids":["g0"]}]},"footprint":["src/lib.rs"],"attempt":1,"round":0,"tests":{"image":format!("sha256:{}","a".repeat(64)),"tools_path":"/tools"},"context_size":1000})
        }
        Role::Reviewer => {
            json!({"goals":[{"id":"g0","description":"Correct behavior","verification":"test"}],"base_sha":"a".repeat(40),"head_sha":"b".repeat(40),"diff":"","checks":[],"standing_rules":[]})
        }
        Role::Classifier => {
            json!({"comment":{"author":"requester","body":"Stop"},"state":{"phase":"maker","closed":false}})
        }
        Role::Writer => {
            json!({"kind":"note","facts":{"issue_title":"Fix","request":"A request","change":"Done"}})
        }
        Role::Postmortem => {
            json!({"record":{"id":1,"trigger":"completed","reason":"Done"},"case":{"request":{},"outcome":"done","grades":[],"phases":[],"questions":[],"incidents":[],"release":"r1"},"rubric":{"version":1,"dimensions":{}},"release":"r1","grade_model":"model","deadline_secs":1,"trace_tags":{"node":"1","request":"1","repo":"owner/repo","issue":"1"}})
        }
    }
}

#[test]
fn new_reply_roots_enforce_generated_boundary_cases() {
    use tl3_contracts::roles::postmortem_reply::Recommendation;
    for n in 1..8 {
        assert!(
            Audit::<Value>::validate_reply(
                json!({"verdict":"agree","objections":vec!["objection";n]})
            )
            .is_err()
        );
        assert!(
            Audit::<Value>::validate_reply(
                json!({"verdict":"disagree","objections":vec!["objection";n]})
            )
            .is_ok()
        );
        let mut planner = json!({"kind":"leaf","files":[],"checks":[{"kind":"regression","node_ids":[],"goal_ids":[]}],"no_design_questions":true,"lands_on_its_own":true});
        for field in ["forcing", "proof", "declared_symptom"] {
            planner["checks"][0][field] = Value::Null;
            assert!(Planner::<Value>::validate_reply(planner.clone()).is_err());
            planner["checks"][0].as_object_mut().unwrap().remove(field);
        }
        let approve = json!({"verdict":"approve","head_sha":"a".repeat(if n%2==0 {40}else{64}),"blocking_findings":[],"findings":[]});
        Reviewer::<Value>::validate_reply(approve.clone()).unwrap();
        let mut invalid = approve;
        invalid["head_sha"] = json!("a".repeat(n));
        assert!(Reviewer::<Value>::validate_reply(invalid).is_err());
        for (operation, before, after) in [
            ("add", "", "new"),
            ("remove", "old", ""),
            ("replace", "old", "new"),
        ] {
            let valid = json!({"phase":"maker","kind":"code","target":"src/lib.rs","operation":operation,"before":before.repeat(n),"after":after.repeat(n),"evidence":"observed","effect":"repair"});
            serde_json::from_value::<Recommendation>(valid.clone()).unwrap();
            let mut invalid = valid;
            let field = if operation == "add" {
                "before"
            } else {
                "after"
            };
            invalid[field] = json!(if operation == "replace" {
                ""
            } else {
                "invalid"
            });
            assert!(serde_json::from_value::<Recommendation>(invalid).is_err());
        }
    }
}
