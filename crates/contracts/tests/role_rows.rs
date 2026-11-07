//! #884/#929: exercise resolved refusal rows through their public interface.
use serde_json::{Value, json};
use tl3_contracts::rules::*;

fn terminal(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|v| v.get("kind") == Some(&json!("cant_meet")))
}
const TERMINALS: &[TerminalForm] = &[
    TerminalForm {
        role: Role::Maker,
        predicate: terminal,
        phrase: "Return kind=cant_meet.",
    },
    TerminalForm {
        role: Role::Planner,
        predicate: terminal,
        phrase: "Return kind=cant_meet.",
    },
];
fn policy() -> Policy<'static> {
    Policy {
        terminals: TERMINALS,
        source_editors: &[Role::Maker],
        schema_membership: None,
    }
}
fn allowed() -> Allowed {
    Allowed::Finite {
        values: vec![json!("good")],
    }
}
fn row() -> Rule<Value> {
    Rule {
        id: "reply.value",
        check: |v| {
            if v["value"] == "good" {
                vec![]
            } else {
                vec![v.clone()]
            }
        },
        names: |_| {
            vec![Name {
                kind: NameKind::Field,
                value: "/value".into(),
            }]
        },
        bad_value: |v| v["value"].clone(),
        allowed: Callback::Facts(|_| allowed()),
        repair: Callback::Role(|_, role| {
            if role == Role::Maker {
                Repair::Edit {
                    pointer: "/value".to_owned().try_into().unwrap(),
                    op: EditOp::Replace,
                    allowed: allowed(),
                }
            } else {
                Repair::CantMeet {
                    reason: "no permitted edit".into(),
                }
            }
        }),
        gating: false,
        roles: vec![Role::Maker, Role::Planner],
    }
}
fn registered() -> Rule<Value> {
    row()
        .register(
            &Registration {
                example: json!({"value":"bad"}),
                reply: json!({"value":"bad"}),
                with_reply: |_, reply| reply.clone(),
                witness: Witness::ByRole(
                    [
                        (Role::Maker, json!({"value":"good"})),
                        (Role::Planner, json!({"kind":"cant_meet"})),
                    ]
                    .into(),
                ),
            },
            &policy(),
        )
        .unwrap()
}

#[test]
fn callbacks_resolve_per_role_and_preserve_wire_identity() {
    let rule = registered();
    for role in [Role::Maker, Role::Planner] {
        let rows = rule
            .evaluate(&json!({"value":"bad"}), role, &policy())
            .unwrap();
        assert_eq!(rows.len(), 1);
        let encoded = serde_json::to_value(&rows).unwrap();
        assert_eq!(encoded[0]["role"], role.as_str());
        assert!(encoded[0].get("evidence").is_none());
        assert_eq!(
            serde_json::from_value::<Vec<Violation>>(encoded).unwrap(),
            rows
        );
        let rendered = render_all(&rows, role, RenderContext::Reply, &policy()).unwrap();
        assert!(rendered.contains("/value"));
        assert!(rendered.contains("good"));
        if role == Role::Maker {
            assert!(
                permits(
                    &rows,
                    &json!({"value":"bad"}),
                    &json!({"value":"good"}),
                    role,
                    &policy()
                )
                .unwrap()
            );
            assert!(
                !permits(
                    &rows,
                    &json!({"value":"bad","other":1}),
                    &json!({"value":"good","other":2}),
                    role,
                    &policy()
                )
                .unwrap()
            );
            assert!(render_all(&rows, role, RenderContext::Tool, &policy()).is_err());
        } else {
            assert!(
                !render_all(&rows, role, RenderContext::Tool, &policy())
                    .unwrap()
                    .contains("Return")
            );
        }
    }
}

#[test]
fn explicit_witness_mapping_checks_each_mixed_repair_role() {
    let mut rule = row();
    rule.repair = Callback::Role(|_, role| {
        if role == Role::Maker {
            Repair::SourceEdit {
                path: "src/lib.rs".into(),
                instruction: "Fix the assertion".into(),
            }
        } else {
            Repair::CantMeet {
                reason: "source tools unavailable".into(),
            }
        }
    });
    rule.names = |_| {
        vec![Name {
            kind: NameKind::Path,
            value: "src/lib.rs".into(),
        }]
    };
    let fixture = Registration {
        example: json!({"value":"bad"}),
        reply: json!({"value":"bad"}),
        with_reply: |_, reply| reply.clone(),
        witness: Witness::ByRole(
            [
                (Role::Maker, Value::Null),
                (Role::Planner, json!({"kind":"cant_meet"})),
            ]
            .into(),
        ),
    };
    let rule = rule.register(&fixture, &policy()).unwrap();
    let rows = rule
        .evaluate(&fixture.example, Role::Maker, &policy())
        .unwrap();
    assert_eq!(
        render_all(&rows, Role::Maker, RenderContext::Reply, &policy()).unwrap(),
        render_all(&rows, Role::Maker, RenderContext::Tool, &policy()).unwrap()
    );
    assert!(
        permits(
            &rows,
            &fixture.reply,
            &fixture.reply,
            Role::Maker,
            &policy()
        )
        .unwrap()
    );
    assert!(
        !permits(
            &rows,
            &fixture.reply,
            &json!({"value":"good"}),
            Role::Maker,
            &policy()
        )
        .unwrap()
    );
    assert!(
        rule.evaluate(&fixture.example, Role::Audit, &policy())
            .is_err()
    );
}

#[test]
fn invalid_context_is_rejected_even_without_violations() {
    assert!(serde_json::from_value::<RenderContext>(json!("unknown")).is_err());
    assert_eq!(RenderContext::default(), RenderContext::Reply);
    assert_eq!(
        render_all(&[], Role::Maker, RenderContext::Reply, &policy()).unwrap(),
        ""
    );
}

#[test]
fn field_pointer_parsing_preserves_escaped_generated_segments() {
    for count in 1..80 {
        let value = format!("/checks/{count}/{}", "~0~1".repeat(count));
        let pointer: FieldPointer = serde_json::from_value(json!(value)).unwrap();
        assert_eq!(pointer.as_str(), value);
        for suffix in ["~", "~2", "~x"] {
            assert!(
                serde_json::from_value::<FieldPointer>(json!(format!("{value}{suffix}"))).is_err()
            );
        }
    }
    for invalid in ["relative", "#/value"] {
        assert!(serde_json::from_value::<FieldPointer>(json!(invalid)).is_err());
    }
}

#[test]
fn remaining_role_roots_join_generated_inventory_without_changing_bytes() {
    let generated = tl3_contracts::generated();
    for (role, committed) in [
        (
            "audit",
            include_str!("../../../workers/src/tl3_workers/schemas/audit-reply.json"),
        ),
        (
            "planner",
            include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json"),
        ),
        (
            "reviewer",
            include_str!("../../../workers/src/tl3_workers/schemas/reviewer-reply.json"),
        ),
        (
            "postmortem",
            include_str!("../../../workers/src/tl3_workers/schemas/postmortem-reply.json"),
        ),
    ] {
        let file = generated
            .iter()
            .find(|file| file.path.ends_with(&format!("/{role}-reply.json")))
            .unwrap_or_else(|| panic!("missing Rust-owned {role} reply"));
        assert_eq!(
            file.text, committed,
            "{role} schema bytes and prompt identity"
        );
    }
}

#[test]
fn evidence_bounds_are_explicit_durable_and_utf8_safe() {
    let mut row = registered()
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
        .unwrap()
        .remove(0);
    let sha: tl3_contracts::bounded::CommitSha =
        tl3_contracts::bounded::CommitSha::new("a".repeat(40)).unwrap();
    let failure = Evidence::TestFailure {
        target: "//pkg:test".into(),
        test: "arbitrary backend id [case/7]".into(),
        excerpt: "é中".repeat(2000),
        log_ref: "log://failure".into(),
        receipt: "sha256:receipt".into(),
    };
    row.evidence = vec![
        Evidence::Attribution {
            batch_head: sha.clone(),
            member: "member".into(),
            member_head: sha.clone(),
            method: AttributionMethod::OutsideSelection {
                failing_targets: (0..70).map(|n| format!("//test:{n}")).collect(),
                failing_targets_truncated: 2,
            },
        },
        Evidence::Attribution {
            batch_head: sha.clone(),
            member: "member".into(),
            member_head: sha.clone(),
            method: AttributionMethod::PrefixBisection {
                passed_prefix_head: sha.clone(),
                failed_prefix_head: sha,
            },
        },
    ];
    row.evidence.extend(std::iter::repeat_n(failure, 18));
    row.evidence_truncated = 3;
    let direct_render = render_all(
        std::slice::from_ref(&row),
        Role::Maker,
        RenderContext::Reply,
        &policy(),
    )
    .unwrap();
    let encoded = serde_json::to_value(&row).unwrap();
    assert_eq!(
        encoded["evidence"].as_array().unwrap().len(),
        EVIDENCE_ITEMS
    );
    assert_eq!(encoded["evidence_truncated"], 7);
    assert_eq!(
        encoded["evidence"][0]["method"]["failing_targets_truncated"],
        8
    );
    assert_eq!(
        encoded["evidence"][0]["method"]["failing_targets"]
            .as_array()
            .unwrap()
            .len(),
        FAILING_TARGETS
    );
    let excerpt = encoded["evidence"][2]["excerpt"].as_str().unwrap();
    assert!(excerpt.len() <= EXCERPT_BYTES);
    assert!(excerpt.ends_with(TRUNCATED));
    let restored: Violation = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), encoded);
    let rendered = render_all(&[restored], Role::Maker, RenderContext::Reply, &policy()).unwrap();
    assert_eq!(rendered, direct_render);
    for expected in [
        "//pkg:test",
        "arbitrary backend id",
        "[truncated]",
        "8 failing targets omitted",
        "7 evidence items omitted",
        "prefix bisection",
    ] {
        assert!(rendered.contains(expected), "{expected}");
    }
    let mut invalid = encoded;
    invalid["role"] = Value::Null;
    assert!(serde_json::from_value::<Violation>(invalid).is_err());
}

#[test]
fn source_edit_subject_path_and_instruction_bounds_are_enforced() {
    let rule = registered();
    let mut row = rule
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
        .unwrap()
        .remove(0);
    row.names = vec![Name {
        kind: NameKind::Symbol,
        value: "src/lib.rs::module.test".into(),
    }];
    row.repair = Repair::SourceEdit {
        path: "src/lib.rs".into(),
        instruction: "Fix\n  the\tassertion".into(),
    };
    let rendered = render_all(
        std::slice::from_ref(&row),
        Role::Maker,
        RenderContext::Tool,
        &policy(),
    )
    .unwrap();
    assert!(rendered.ends_with("Change \"src/lib.rs\": Fix\n  the\tassertion"));
    for (path, instruction) in [
        ("src/other.rs", "Fix it".to_owned()),
        ("../escape.rs", "Fix it".into()),
        ("src/lib.rs", " ".into()),
        ("src/lib.rs", "é".repeat(INSTRUCTION_BYTES)),
    ] {
        row.repair = Repair::SourceEdit {
            path: path.into(),
            instruction,
        };
        assert!(row.validate(Role::Maker, &policy()).is_err());
    }
    for value in [
        "//pkg:test",
        "tests/test_x.py::case[7]",
        "src/lib.rs::",
        "../src/lib.rs::test",
    ] {
        assert!(
            Name {
                kind: NameKind::Symbol,
                value: value.into()
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn json_types_and_const_wording_survive_the_shared_form() {
    let mut row = registered()
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
        .unwrap()
        .remove(0);
    for value in [
        json!(true),
        json!(1),
        json!("1"),
        Value::Null,
        json!([1]),
        json!({"one":1}),
    ] {
        row.allowed = Allowed::Schema {
            keyword: SchemaKeyword::Const,
            value: value.clone(),
        };
        row.repair = Repair::Edit {
            pointer: "/value".to_owned().try_into().unwrap(),
            op: EditOp::Replace,
            allowed: row.allowed.clone(),
        };
        assert!(
            permits(
                std::slice::from_ref(&row),
                &json!({"value":"bad"}),
                &json!({"value":value}),
                Role::Maker,
                &policy()
            )
            .unwrap()
        );
        let text = render_all(
            std::slice::from_ref(&row),
            Role::Maker,
            RenderContext::Reply,
            &policy(),
        )
        .unwrap();
        assert!(text.ends_with("with the allowed value above."));
        let different = if value == json!(true) {
            json!(1)
        } else {
            json!(true)
        };
        assert!(
            !permits(
                std::slice::from_ref(&row),
                &json!({"value":"bad"}),
                &json!({"value":different}),
                Role::Maker,
                &policy()
            )
            .unwrap()
        );
    }
}

#[test]
fn root_pointer_and_independent_edits_keep_their_permissions() {
    let policy = policy();
    let base = registered()
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy)
        .unwrap()
        .remove(0);
    let mut root = base.clone();
    root.names[0].value = String::new();
    root.repair = Repair::Edit {
        pointer: String::new().try_into().unwrap(),
        op: EditOp::Replace,
        allowed: Allowed::Finite {
            values: vec![json!({"value":"good"})],
        },
    };
    assert!(
        permits(
            &[root],
            &json!({"value":"bad"}),
            &json!({"value":"good"}),
            Role::Maker,
            &policy
        )
        .unwrap()
    );
    let mut other = base.clone();
    other.names[0].value = "/other".into();
    other.repair = Repair::Edit {
        pointer: "/other".to_owned().try_into().unwrap(),
        op: EditOp::Add,
        allowed: allowed(),
    };
    let rows = [base, other];
    assert!(
        permits(
            &rows,
            &json!({"value":"bad"}),
            &json!({"value":"good","other":"good"}),
            Role::Maker,
            &policy
        )
        .unwrap()
    );
    assert!(
        !permits(
            &rows,
            &json!({"value":"bad"}),
            &json!({"value":"good","other":true}),
            Role::Maker,
            &policy
        )
        .unwrap()
    );
    let many: Vec<_> = (0..1000).map(|_| rows[0].clone()).collect();
    let rendered = render_all(&many, Role::Maker, RenderContext::Reply, &policy).unwrap();
    assert_eq!(rendered.matches("reply.value:").count(), 1000);
    assert!(
        rendered.chars().count() > 65536,
        "the adapter owns the fail-loud limit; no whole-message clipping here"
    );
}

#[test]
fn callback_witness_maps_and_all_advertised_alternatives_are_checked() {
    let mut fixture = Registration {
        example: json!({"value":"bad"}),
        reply: json!({"value":"bad"}),
        with_reply: |_, reply| reply.clone(),
        witness: Witness::Shared(json!({"value":"good"})),
    };
    assert!(row().register(&fixture, &policy()).is_err());
    fixture.witness = Witness::ByRole([(Role::Maker, json!({"value":"good"}))].into());
    assert!(row().register(&fixture, &policy()).is_err());
    fixture.witness = Witness::ByRole(
        [
            (Role::Maker, json!({"value":"good"})),
            (Role::Planner, json!({"kind":"cant_meet"})),
        ]
        .into(),
    );
    let mut invalid = row();
    invalid.repair = Callback::Static(Repair::Edit {
        pointer: "/value".to_owned().try_into().unwrap(),
        op: EditOp::Replace,
        allowed: Allowed::Finite {
            values: vec![json!("good"), json!("still bad")],
        },
    });
    invalid.roles = vec![Role::Maker];
    assert!(invalid.register(&fixture, &policy()).is_err());
}

#[test]
fn schema_membership_is_an_explicit_adapter_and_callbacks_stay_off_wire() {
    let mut policy = policy();
    let mut rows = registered()
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy)
        .unwrap();
    rows[0].repair = Repair::Edit {
        pointer: "/value".to_owned().try_into().unwrap(),
        op: EditOp::Replace,
        allowed: Allowed::Schema {
            keyword: SchemaKeyword::MinLength,
            value: json!(3),
        },
    };
    assert!(
        permits(
            &rows,
            &json!({"value":"bad"}),
            &json!({"value":"good"}),
            Role::Maker,
            &policy
        )
        .is_err()
    );
    policy.schema_membership = Some(|keyword, limit, value| {
        assert_eq!(keyword, SchemaKeyword::MinLength);
        Ok(value
            .as_str()
            .is_some_and(|text| text.chars().count() >= limit.as_u64().unwrap() as usize))
    });
    assert!(
        permits(
            &rows,
            &json!({"value":"bad"}),
            &json!({"value":"good"}),
            Role::Maker,
            &policy
        )
        .unwrap()
    );
    assert!(
        !permits(
            &rows,
            &json!({"value":"bad"}),
            &json!({"value":true}),
            Role::Maker,
            &policy
        )
        .unwrap()
    );
    assert!(!serde_json::to_string(&rows).unwrap().contains("callback"));
    assert!(serde_json::from_value::<Role>(json!("unresolved")).is_err());
    assert!(
        serde_json::from_value::<Name>(json!({"kind":"symbol","value":"backend[test]"})).is_err()
    );
}

#[test]
fn unicode_source_identifiers_reject_backend_ids_and_invalid_starts() {
    for path in ["tests/test_api.py", "src/lib.rs"] {
        for name in [
            "test_é",
            "_private",
            "Δοκιμή",
            "cafe\u{301}",
            "module.test_名",
        ] {
            let subject = Name {
                kind: NameKind::Symbol,
                value: format!("{path}::{name}"),
            };
            subject.validate().unwrap();
            assert_eq!(
                serde_json::from_value::<Name>(serde_json::to_value(&subject).unwrap()).unwrap(),
                subject
            );
        }
        for name in [
            "2start",
            "\u{301}start",
            "test²",
            "a.7bad",
            "case[0]",
            "",
            "name::other",
        ] {
            assert!(
                Name {
                    kind: NameKind::Symbol,
                    value: format!("{path}::{name}")
                }
                .validate()
                .is_err(),
                "{name}"
            );
        }
    }
}

#[test]
fn root_null_edit_and_source_repair_recheck_reply_only() {
    fn mixed() -> Rule<Value> {
        Rule {
            id: "mixed.repair",
            check: |facts| {
                let mut source = facts.clone();
                source["subject"] = json!("source");
                let mut rows = vec![source];
                if !facts["reply"].is_null() {
                    let mut edit = facts.clone();
                    edit["subject"] = json!("reply");
                    rows.push(edit);
                }
                rows
            },
            names: |facts| {
                if facts["subject"] == "source" {
                    vec![Name {
                        kind: NameKind::Path,
                        value: "src/lib.rs".into(),
                    }]
                } else {
                    vec![Name {
                        kind: NameKind::Field,
                        value: String::new(),
                    }]
                }
            },
            bad_value: |facts| facts["reply"].clone(),
            allowed: Callback::Static(Allowed::Schema {
                keyword: SchemaKeyword::Const,
                value: Value::Null,
            }),
            repair: Callback::Facts(|facts| {
                if facts["subject"] == "source" {
                    Repair::SourceEdit {
                        path: "src/lib.rs".into(),
                        instruction: "Fix source".into(),
                    }
                } else {
                    Repair::Edit {
                        pointer: String::new().try_into().unwrap(),
                        op: EditOp::Replace,
                        allowed: Allowed::Schema {
                            keyword: SchemaKeyword::Const,
                            value: Value::Null,
                        },
                    }
                }
            }),
            gating: false,
            roles: vec![Role::Maker],
        }
    }
    let fixture = Registration {
        example: json!({"reply":{"broken":true}}),
        reply: json!({"broken":true}),
        with_reply: |facts, reply| {
            let mut facts = facts.clone();
            facts["reply"] = reply.clone();
            facts
        },
        witness: Witness::Shared(Value::Null),
    };
    mixed().register(&fixture, &policy()).unwrap();
}

#[test]
fn empty_evidence_preserves_reply_rendering_and_adapter_errors_are_not_permissions() {
    let rule = registered();
    let mut rows = rule
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
        .unwrap();
    assert_eq!(
        render_all(&rows, Role::Maker, RenderContext::Reply, &policy()).unwrap(),
        "reply.value: reply field \"/value\". Bad value: \"bad\". Allowed: enum [\"good\"]. Permitted edit: replace reply field \"/value\" with one of the allowed values above."
    );
    rows[0].repair = Repair::Edit {
        pointer: "/value".to_owned().try_into().unwrap(),
        op: EditOp::Replace,
        allowed: Allowed::Schema {
            keyword: SchemaKeyword::Schema,
            value: json!({"minLength":3}),
        },
    };
    let mut adapter = policy();
    adapter.schema_membership = Some(|_, _, _| Err("validator unavailable".into()));
    assert_eq!(
        permits(
            &rows,
            &json!({"value":"bad"}),
            &json!({"value":"good"}),
            Role::Maker,
            &adapter
        ),
        Err("validator unavailable".into())
    );
    let mut registration = row();
    registration.roles = vec![Role::Maker];
    registration.repair = Callback::Static(rows.remove(0).repair);
    let fixture = Registration {
        example: json!({"value":"bad"}),
        reply: json!({"value":"bad"}),
        with_reply: |_, reply| reply.clone(),
        witness: Witness::ByRole([(Role::Maker, json!({"value":"good"}))].into()),
    };
    assert_eq!(
        registration.register(&fixture, &adapter).err().unwrap(),
        "validator unavailable"
    );
}

#[test]
fn mixed_field_and_source_repairs_register_a_complete_reply_witness() {
    let rule = Rule {
        id: "mixed.field_source",
        check: |facts: &Value| {
            let mut rows = vec![json!({"source":true})];
            if facts["value"] != "good" {
                rows.push(json!({"source":false}));
            }
            rows
        },
        names: |facts| {
            vec![Name {
                kind: if facts["source"] == true {
                    NameKind::Path
                } else {
                    NameKind::Field
                },
                value: if facts["source"] == true {
                    "src/lib.rs"
                } else {
                    "/value"
                }
                .into(),
            }]
        },
        bad_value: |_| Value::Null,
        allowed: Callback::Static(Allowed::Schema {
            keyword: SchemaKeyword::Const,
            value: json!("good"),
        }),
        repair: Callback::Facts(|facts| {
            if facts["source"] == true {
                Repair::SourceEdit {
                    path: "src/lib.rs".into(),
                    instruction: "Fix the source assertion.".into(),
                }
            } else {
                Repair::Edit {
                    pointer: "/value".to_owned().try_into().unwrap(),
                    op: EditOp::Replace,
                    allowed: Allowed::Schema {
                        keyword: SchemaKeyword::Const,
                        value: json!("good"),
                    },
                }
            }
        }),
        gating: false,
        roles: vec![Role::Maker],
    };
    let fixture = Registration {
        example: json!({"value":"bad"}),
        reply: json!({"value":"bad"}),
        with_reply: |_, reply| reply.clone(),
        witness: Witness::Shared(json!({"value":"good"})),
    };
    let registered = rule.register(&fixture, &policy()).unwrap();
    let rows = registered
        .evaluate(&fixture.example, Role::Maker, &policy())
        .unwrap();
    assert!(
        permits(
            &rows,
            &fixture.reply,
            &json!({"value":"good"}),
            Role::Maker,
            &policy()
        )
        .unwrap()
    );
    assert!(
        !permits(
            &rows,
            &fixture.reply,
            &json!({"value":"good", "other":true}),
            Role::Maker,
            &policy()
        )
        .unwrap()
    );
}

#[test]
fn source_instruction_wire_boundary_counts_utf8_bytes() {
    for instruction in [
        "a".repeat(INSTRUCTION_BYTES),
        "é".repeat(INSTRUCTION_BYTES / 2),
    ] {
        let repair: Repair = serde_json::from_value(
            json!({"kind":"source_edit", "path":"src/lib.rs", "instruction":instruction}),
        )
        .unwrap();
        let mut row = registered()
            .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
            .unwrap()
            .remove(0);
        row.names = vec![Name {
            kind: NameKind::Path,
            value: "src/lib.rs".into(),
        }];
        row.repair = repair;
        assert!(row.validate(Role::Maker, &policy()).is_ok());
    }
    for instruction in [
        "a".repeat(INSTRUCTION_BYTES + 1),
        "é".repeat(INSTRUCTION_BYTES / 2) + "a",
    ] {
        assert!(
            serde_json::from_value::<Repair>(
                json!({"kind":"source_edit", "path":"src/lib.rs", "instruction":instruction})
            )
            .is_err()
        );
    }
}

#[test]
fn source_editor_and_terminal_authority_are_required() {
    let mut row = registered()
        .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
        .unwrap()
        .remove(0);
    row.names = vec![Name {
        kind: NameKind::Path,
        value: "src/lib.rs".into(),
    }];
    row.repair = Repair::SourceEdit {
        path: "src/lib.rs".into(),
        instruction: "Fix it.".into(),
    };
    let no_editors = Policy {
        source_editors: &[],
        ..policy()
    };
    assert!(row.validate(Role::Maker, &no_editors).is_err());
    assert!(
        render_all(
            std::slice::from_ref(&row),
            Role::Maker,
            RenderContext::Reply,
            &no_editors
        )
        .is_err()
    );
    row.role = Role::Postmortem;
    row.repair = Repair::CantMeet {
        reason: "No valid repair.".into(),
    };
    assert!(row.validate(Role::Postmortem, &policy()).is_err());
    assert!(render_all(&[row], Role::Postmortem, RenderContext::Reply, &policy()).is_err());
}

#[test]
fn excerpt_exact_byte_boundary_and_straddling_character_are_pinned() {
    for (excerpt, expected) in [
        ("é".repeat(EXCERPT_BYTES / 2), "é".repeat(EXCERPT_BYTES / 2)),
        (
            "a".repeat(EXCERPT_BYTES - TRUNCATED.len() - 1) + "💡" + &"z".repeat(32),
            "a".repeat(EXCERPT_BYTES - TRUNCATED.len() - 1) + TRUNCATED,
        ),
    ] {
        let mut row = registered()
            .evaluate(&json!({"value":"bad"}), Role::Maker, &policy())
            .unwrap()
            .remove(0);
        row.evidence = vec![Evidence::TestFailure {
            target: "//pkg:test".into(),
            test: "case".into(),
            excerpt,
            log_ref: "log".into(),
            receipt: "receipt".into(),
        }];
        let encoded = serde_json::to_value(&row).unwrap();
        assert_eq!(encoded["evidence"][0]["excerpt"], expected);
        let restored: Violation = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), encoded);
    }
}
