use super::*;
use serde_json::json;

fn checked_plan_reply() -> Value {
    json!({"kind":"cant_meet", "report":"Named plan-check objections remain.",
        "files":["greeting.py"], "forecast":ingest(1)["forecast"],
        "violations":["A named rule remains unsatisfied."]})
}

fn plan_check_provenance(
    class: crate::contracts::plan_check::ObjectionClass,
) -> crate::contracts::plan_check::PlanCheckProvenance {
    use crate::contracts::plan_check::*;
    PlanCheckProvenance {
        objections: vec![PlanObjection {
            class,
            rule: "check.named_rule".into(),
            items: vec!["check-1".into()],
            diagnostic: "The original check obligation remains unsatisfied.".into(),
        }],
        repair: PlanRepair::Attempted,
    }
}

fn planning_world(fail_fast: bool, retries: u8) -> tl3_kernel::World {
    use tl3_kernel::*;
    fn confirm(world: &mut World) {
        for _ in 0..32 {
            let Some(effect) = world.nodes[0].effect else {
                return;
            };
            *world = step(
                world.clone(),
                Event::Ack {
                    node: NodeId(0),
                    seq: effect.seq,
                    ack: Ack::Confirmed,
                },
            )
            .0;
        }
        panic!("fixture did not finish confirming effects");
    }
    let mut world = World::new(&[1], 1);
    world.fail_fast = fail_fast;
    world = step(
        world,
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    )
    .0;
    confirm(&mut world);
    for reply in [Reply::IngestDone { size: 1 }, Reply::AuditAgrees] {
        let generation = world.nodes[0].generation;
        world = step(
            world,
            Event::Reply {
                node: NodeId(0),
                generation,
                reply,
            },
        )
        .0;
        confirm(&mut world);
    }
    world.nodes[0].plan_retries = retries;
    assert!(matches!(world.nodes[0].phase, Phase::Planning { .. }));
    assert!(world.valid());
    world
}

fn assert_plan_check_stops(class: crate::contracts::plan_check::ObjectionClass) {
    use crate::contracts::plan_check::*;
    use tl3_kernel::*;
    for fail_fast in [false, true] {
        for retries in [0, 1] {
            for repair in [
                PlanRepair::Attempted,
                PlanRepair::Unavailable {
                    reason: RepairUnavailable::Disabled,
                },
                PlanRepair::Unavailable {
                    reason: RepairUnavailable::InputAllowance,
                },
                PlanRepair::Unavailable {
                    reason: RepairUnavailable::MessageLimit,
                },
                PlanRepair::Unavailable {
                    reason: RepairUnavailable::FormatAllowanceConsumed,
                },
            ] {
                let mut provenance = plan_check_provenance(class);
                provenance.repair = repair;
                let MappedReply::Kernel(reply) =
                    map_reply_with_plan_check("planner", &checked_plan_reply(), Some(&provenance))
                        .unwrap()
                else {
                    panic!("expected kernel reply")
                };
                let before = planning_world(fail_fast, retries);
                let generation = before.nodes[0].generation;
                let (after, out) = step(
                    before.clone(),
                    Event::Reply {
                        node: NodeId(0),
                        generation,
                        reply,
                    },
                );
                assert!(
                    matches!(
                        after.nodes[0].phase,
                        Phase::Stopping(Outcome::Stopped) | Phase::Closed(Outcome::Stopped)
                    ),
                    "fail_fast={fail_fast}, retries={retries}, repair={repair:?}: {:?}",
                    after.nodes[0].phase
                );
                assert_eq!(after.nodes[0].review_stop, Some(ReviewStop::PlanCantMeet));
                assert_eq!(after.nodes[0].question, None);
                assert_eq!(after.next_question, before.next_question);
                assert_eq!(after.nodes[0].credit, before.nodes[0].credit);
                assert_eq!(after.nodes[0].plan_retries, retries);
                assert!(!out.iter().any(|out| matches!(
                    out,
                    Out::Notice {
                        notice: Notice::Ask { .. },
                        ..
                    }
                )));
                assert!(after.valid());
            }
        }
    }
}

#[test]
fn plan_check_rejected_never_asks_or_retries() {
    assert_plan_check_stops(crate::contracts::plan_check::ObjectionClass::Other);
}

#[test]
fn plan_check_needs_split_never_asks_or_retries() {
    assert_plan_check_stops(crate::contracts::plan_check::ObjectionClass::SizeExecutionFit);
}

#[test]
fn plan_check_without_provenance_preserves_the_legacy_fallback() {
    assert!(matches!(
        map_reply("planner", &checked_plan_reply()).unwrap(),
        MappedReply::Kernel(Reply::PlanCantMeet)
    ));
    let mut forged = checked_plan_reply();
    forged["plan_check"] = serde_json::to_value(plan_check_provenance(
        crate::contracts::plan_check::ObjectionClass::Other,
    ))
    .unwrap();
    assert!(map_reply("planner", &forged).is_err());
}

#[test]
fn plan_check_keeps_ordinary_split_and_cant_meet_owner_paths() {
    use tl3_kernel::*;
    for (reply, kind) in [
        (Reply::PlanSplit, QKind::NeedsSplitting),
        (Reply::PlanCantMeet, QKind::CantMeet),
    ] {
        let before = planning_world(false, 1);
        let generation = before.nodes[0].generation;
        let (after, out) = step(
            before,
            Event::Reply {
                node: NodeId(0),
                generation,
                reply,
            },
        );
        assert_eq!(after.nodes[0].phase, Phase::AwaitingRob);
        assert_eq!(after.nodes[0].question.unwrap().kind, kind);
        assert!(out.iter().any(|out| matches!(
            out,
            Out::Notice {
                notice: Notice::Ask { .. },
                ..
            }
        )));
    }
}

#[test]
fn plan_check_event_metadata_survives_without_changing_the_model_reply() {
    use crate::contracts::plan_check::*;
    let delivery: crate::executor::Delivery = serde_json::from_value(json!({
        "key":"plan-check", "epoch":1, "generation":3,
        "output":{"Effect":{"node":0,"kind":{"StartRun":"Plan"},"seq":1,"head":null}},
        "head_sha":null,"repository":{"owner":"owner","name":"repo","installation":1},
        "issue":832,"pull_request":null,"paths":["greeting.py"],"attempt":1,"round":0,"checks":null
    }))
    .unwrap();
    let inputs = json!({"protected_paths":[], "forecast":{
        "fit_tokens":100,"context_tokens":150,"overhead_tokens":{"prompt":0}}});
    for class in [ObjectionClass::Other, ObjectionClass::SizeExecutionFit] {
        let provenance = plan_check_provenance(class);
        let reply = checked_plan_reply();
        let bytes = format!(
            "{}\n",
            json!({"event":"reply", "role":"planner",
            "reply":reply, "plan_check":provenance})
        );
        let crate::executor::Observation::WorkerReply {
            reply: checked,
            plan_check,
            ..
        } = super::super::source::parse_reply(bytes.as_bytes(), &delivery, &inputs).unwrap()
        else {
            panic!("expected worker reply")
        };
        assert_eq!(plan_check.as_ref(), Some(&provenance));
        assert!(checked.get("plan_check").is_none());
        assert!(matches!(
            map_reply_with_plan_check("planner", &checked, plan_check.as_ref()).unwrap(),
            MappedReply::Kernel(Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
        ));
    }
}

#[test]
fn plan_check_mixed_objections_stop_and_empty_provenance_is_invalid() {
    use crate::contracts::plan_check::*;
    let mut provenance = plan_check_provenance(ObjectionClass::SizeExecutionFit);
    provenance
        .objections
        .extend(plan_check_provenance(ObjectionClass::Other).objections);
    assert!(matches!(
        map_reply_with_plan_check("planner", &checked_plan_reply(), Some(&provenance)).unwrap(),
        MappedReply::Kernel(Reply::PlanCheckRejected)
    ));
    provenance.objections.clear();
    assert!(
        map_reply_with_plan_check("planner", &checked_plan_reply(), Some(&provenance)).is_err()
    );
}

fn ingest(tokens: u64) -> Value {
    json!({
        "kind": "brief", "brief": "Fix greeting.",
        "goals": [{"description": "Return hello.", "verification": "test"}],
        "memory_ids": [], "files": ["greeting.py"],
        "forecast": {
            "tokenizer": "o200k_base", "brief_tokens": tokens,
            "file_tokens": {"greeting.py": 0}, "overhead_tokens": 0,
            "tokens": tokens, "fit_tokens": 100, "context_tokens": 150,
            "fits": tokens < 100,
        }
    })
}

#[test]
fn sizing_ceil_floor_and_kernel_range() {
    for (tokens, size) in [
        (0, Some(1)),
        (1, Some(1)),
        (100, Some(1)),
        (101, Some(2)),
        (25_500, Some(255)),
        (25_501, None),
    ] {
        let mapped = map_reply("ingest", &ingest(tokens));
        if let Some(size) = size {
            assert!(
                matches!(mapped.unwrap(), MappedReply::Kernel(Reply::IngestDone { size: actual }) if actual == size),
                "{tokens}"
            );
        } else {
            assert!(mapped.is_err(), "{tokens} must fail instead of wrapping");
        }
    }
}

#[test]
fn rejects_inconsistent_or_forged_forecasts() {
    for (field, value) in [
        ("tokens", json!(2)),
        ("fits", json!(false)),
        ("fit_tokens", json!(0)),
        ("context_tokens", json!(100)),
        ("brief_tokens", json!(u64::MAX)),
        ("overhead_tokens", json!(u64::MAX)),
        ("tokenizer", json!("other")),
        ("file_tokens", json!({})),
        ("file_tokens", json!({"forged.py": 0})),
        ("file_tokens", json!({"greeting.py": 0, "extra.py": 0})),
    ] {
        let mut reply = ingest(1);
        reply["forecast"][field] = value;
        assert!(map_reply("ingest", &reply).is_err(), "{field}: {reply}");
    }
}

#[test]
fn checks_forecast_against_saved_configuration() {
    let settings =
        json!({"fit_tokens": 100, "context_tokens": 150, "overhead_tokens": {"prompt": 0}});
    let reply = ingest(1);
    validate_forecast_settings(&reply, &settings).unwrap();
    for (field, value) in [
        ("fit_tokens", 101),
        ("context_tokens", 151),
        ("overhead_tokens", 1),
    ] {
        let mut forged = reply.clone();
        forged["forecast"][field] = json!(value);
        if field == "overhead_tokens" {
            forged["forecast"]["tokens"] = json!(2);
        }
        map_reply("ingest", &forged).unwrap();
        assert!(
            validate_forecast_settings(&forged, &settings).is_err(),
            "{field}"
        );
    }
}

#[test]
fn split_forecast_keys_are_the_union_of_piece_files() {
    let mut reply = json!({"kind": "split", "reason": "out_of_room", "pieces": [
        {"brief": "First", "goal_ids": [0], "files": ["greeting.py"]},
        {"brief": "Second", "goal_ids": [0], "files": ["greeting.py", "new.py"]},
    ], "forecast": ingest(1)["forecast"], "violations": []});
    assert!(map_reply("planner", &reply).is_err());
    reply["forecast"]["file_tokens"]["new.py"] = json!(0);
    assert!(matches!(
        map_reply("planner", &reply).unwrap(),
        MappedReply::Kernel(Reply::PlanSplit)
    ));
    reply["forecast"]["file_tokens"]["extra.py"] = json!(0);
    assert!(map_reply("planner", &reply).is_err());
}

#[test]
fn leaf_with_declared_test_edits_maps_by_its_violations() {
    // #438: edits_tests is optional on a leaf; worker code reports its rule breaks.
    let test = "tests/test_greeting.py::test_hello";
    let mut reply = json!({"kind": "leaf", "files": ["greeting.py"], "checks": [
        {"kind": "demonstrating", "node_ids": [test], "goal_ids": [0]},
    ], "edits_tests": [test], "no_design_questions": true, "lands_on_its_own": true,
    "forecast": ingest(1)["forecast"], "violations": []});
    assert!(matches!(
        map_reply("planner", &reply).unwrap(),
        MappedReply::Kernel(Reply::PlanLeaf { fits: true, .. })
    ));
    reply["violations"] = json!(["A test the change edits can't be frozen."]);
    assert!(matches!(
        map_reply("planner", &reply).unwrap(),
        MappedReply::Kernel(Reply::PlanCantMeet)
    ));
    let mut cant_meet = json!({"kind": "cant_meet", "report": "Blocked.", "files": ["greeting.py"],
        "edits_tests": [test], "forecast": ingest(1)["forecast"], "violations": []});
    assert!(map_reply("planner", &cant_meet).is_err());
    cant_meet.as_object_mut().unwrap().remove("edits_tests");
    assert!(matches!(
        map_reply("planner", &cant_meet).unwrap(),
        MappedReply::Kernel(Reply::PlanCantMeet)
    ));
}

#[test]
fn blocked_maker_reply_needs_cited_evidence() {
    // Worker code adds the class (and tool) it recorded; the controller cites only those.
    for evidence in [
        json!({"action_key": "c".repeat(64), "class": "test_infrastructure"}),
        json!({"tool_error": "tool-error-1", "tool": "run_tests", "class": "test_submission"}),
        json!({"tool_error": "tool-error-2", "tool": "write", "class": "EROFS"}),
    ] {
        let reply =
            json!({"kind": "blocked", "evidence": evidence, "report": "PrismaBuild failed."});
        assert!(
            matches!(
                map_reply("maker", &reply),
                Ok(MappedReply::Kernel(Reply::RunFailed))
            ),
            "{reply}"
        );
    }
    for evidence in [
        json!(null),
        json!({}),
        json!({"action_key": "short", "class": "test_infrastructure"}),
        json!({"action_key": "c".repeat(64)}),
        json!({"action_key": "c".repeat(64), "class": "@octocat"}),
        json!({"tool_error": "anything", "tool": "read", "class": "EIO"}),
        json!({"tool_error": "tool-error-1", "class": "EIO"}),
        json!({"tool_error": "tool-error-1", "tool": "shell", "class": "EIO"}),
        json!({"tool_error": "tool-error-1", "tool": "read", "class": "ENOTDIR"}),
        json!({"action_key": "c".repeat(64), "tool_error": "tool-error-1", "class": "EIO"}),
    ] {
        let mut reply = json!({"kind": "blocked", "report": "Broken."});
        if !evidence.is_null() {
            reply["evidence"] = evidence;
        }
        assert!(map_reply("maker", &reply).is_err(), "{reply}");
    }
}

#[test]
fn configured_evidence_preflight_does_not_accept_a_downgraded_leaf() {
    let delivery: crate::executor::Delivery = serde_json::from_value(json!({
        "key":"evidence-preflight", "epoch":1, "generation":3,
        "output":{"Effect":{"node":0,"kind":{"StartRun":"Plan"},"seq":1,"head":null}},
        "head_sha":null,"repository":{"owner":"owner","name":"repo","installation":1},
        "issue":2640,"pull_request":null,"paths":["greeting.py"],"attempt":1,"round":0,"checks":null
    }))
    .unwrap();
    let contract = json!([{"requirement":"PB2640-2650 and PB2654 require BuildBuddy base/head runs under pinned CPU contention.",
        "tool":"run_tests", "executor":"buildbuddy_remote_bazel", "runner_features":["base_head","pinned_cpu_contention"],
        "artifacts":["invocation_url","sha","logs","terminal_verdict"],
        "methods":["red_green","forced_reproduction.contention_wrapper"], "protected_files":[]}]);
    let inputs = json!({"ingest":{"evidence_contract":contract}, "tool_capabilities":[{
        "tool":"run_tests","executor":"prismabuild_pytest", "runner_features":["base_head"],
        "artifacts":["sha","logs","terminal_verdict"],"methods":["red_green"],"protected_files":[]}],
        "protected_paths":[], "forecast":{"fit_tokens":100,"context_tokens":150,"overhead_tokens":{"prompt":0}}});
    let reply = json!({"kind":"leaf","files":["greeting.py"],"checks":[],
        "no_design_questions":true,"lands_on_its_own":true,"forecast":ingest(1)["forecast"],"violations":[]});
    for proposal in [
        reply,
        json!({"kind":"split","reason":"out_of_room", "pieces":[
        {"brief":"First part", "goal_ids":[0], "files":["greeting.py"]},
        {"brief":"Second part", "goal_ids":[0], "files":["greeting.py"]}],
        "forecast":ingest(1)["forecast"], "violations":[]}),
    ] {
        let bytes = format!(
            "{}\n",
            json!({"event":"reply","role":"planner","reply":proposal})
        );
        let crate::executor::Observation::WorkerReply { reply, .. } =
            super::super::source::parse_reply(bytes.as_bytes(), &delivery, &inputs).unwrap()
        else {
            panic!()
        };
        assert_eq!(reply["kind"], "cant_meet");
        assert!(
            reply["report"]
                .as_str()
                .unwrap()
                .contains("buildbuddy_remote_bazel")
        );
        assert!(matches!(
            map_reply("planner", &reply).unwrap(),
            MappedReply::Kernel(Reply::PlanCantMeet)
        ));
    }
}

#[test]
fn configured_evidence_preflight_accepts_always_tests() {
    let contract = json!([{
        "requirement":"Run the repository's always_tests alongside the planned checks.",
        "tool":"run_tests", "executor":"prismabuild_pytest", "runner_features":["always_tests"],
        "artifacts":["logs","terminal_verdict"], "methods":["head"], "protected_files":[]
    }]);
    let inputs = json!({
        "ingest":{"evidence_contract":contract},
        "tool_capabilities":tl3_config::Planning::default().tool_capabilities
    });
    let mut reply = json!({"kind":"leaf", "evidence_contract":[]});
    super::super::evidence::preflight("planner", &inputs, &mut reply).unwrap();
    assert_eq!(reply["kind"], "leaf", "{reply}");
    assert_eq!(reply["evidence_contract"], contract);
    assert_eq!(reply["evidence_errors"], json!([]));
}

#[test]
fn request_123_pending_is_a_handover_not_a_false_success() {
    let reply = json!({"kind":"done", "head":"a".repeat(40), "self_check":[
        {"goal_id":"repair", "status":"met", "evidence":"Normal head passed."},
        {"goal_id":"publication", "status":"pending", "why":"Publishing owns PR creation."}
    ]});
    assert!(matches!(
        map_reply("maker", &reply).unwrap(),
        MappedReply::Made(_)
    ));
}

#[test]
fn t653_schema_roles_have_controller_contracts() {
    let actual: std::collections::BTreeSet<_> = PACKAGED_SCHEMAS
        .iter()
        .filter_map(|(name, _)| name.strip_suffix("-input.json"))
        .collect();
    assert_eq!(
        actual,
        [
            "ingest",
            "audit",
            "planner",
            "maker",
            "reviewer",
            "classifier",
            "writer",
            "postmortem"
        ]
        .into_iter()
        .collect(),
        "each new input schema needs a production-builder contract case"
    );
}

#[test]
fn t653_input_failure_names_role_pointer_and_rule_without_values() {
    for (inputs, pointer, rule) in [
        (
            json!({"comment":{"author": "owner", "body": "sensitive-value"}, "state":{"phase":23, "closed":false}}),
            "/state/phase",
            "type",
        ),
        (
            json!({"comment":{"author": "owner", "body": "sensitive-value"}, "state":{"phase":"Making", "closed":false, "new_field":"sensitive-value"}}),
            "/state/new_field",
            "additionalProperties",
        ),
        (
            json!({"comment":{"author": "owner", "body": "sensitive-value"}, "state":{"closed":false}}),
            "/state/phase",
            "required",
        ),
    ] {
        let error = validate_inputs("classifier", &inputs).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("role=classifier"), "{message}");
        assert!(message.contains(&format!("pointer={pointer}")), "{message}");
        assert!(message.contains(&format!("rule={rule}")), "{message}");
        assert!(!message.contains("sensitive-value"));
        assert_eq!(crate::looks::failure_class(&error), "worker_input_schema");
    }
}

#[test]
fn t653_composed_writer_failure_names_the_applicable_branch_field() {
    let error = validate_inputs(
        "writer",
        &json!({"kind":"summary","facts":{"issue_title":"private-input-value",
            "request":"private-input-value","outcome":"Delivered"}}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "worker inputs break role schema: role=writer pointer=/facts/unmet_goals rule=required"
    );
}

#[test]
fn t653_nested_anyof_failure_keeps_the_matching_leaf_without_values() {
    let schema = json!({"anyOf":[
        {"properties":{"kind":{"const":"other"},"facts":{"required":["unrelated"]}}},
        {"properties":{"kind":{"const":"summary"},"facts":{"oneOf":[
            {"type":"string"},
            {"type":"object","properties":{"goals":{"type":"array","items":{"type":"integer"}}}}
        ]}}}
    ]});
    let validator = jsonschema::validator_for(&schema).unwrap();
    let inputs = json!({"kind":"summary","facts":{"goals":["private-input-value"]}});
    let error = validator.validate(&inputs).unwrap_err();
    assert_eq!(
        crate::worker_inputs::Failure::capture("writer", &error).to_string(),
        "worker inputs break role schema: role=writer pointer=/facts/goals/0 rule=type"
    );
}
