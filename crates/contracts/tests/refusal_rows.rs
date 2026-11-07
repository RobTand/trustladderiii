//! Public resolved-row cases extracted from dc59 C0; no evaluator or witness engine.
use serde_json::{Value, json};
use tl3_contracts::rules::*;
fn terminal(value: &Value) -> bool { value.get("kind") == Some(&json!("cant_meet")) }
const TERMINALS: &[TerminalForm] = &[
    TerminalForm { role: Role::Planner, predicate: terminal, phrase: "Return kind cant_meet." },
    TerminalForm { role: Role::Maker, predicate: terminal, phrase: "Return kind cant_meet." },
];
fn policy() -> Policy<'static> { Policy { terminals: TERMINALS, source_editors: &[Role::Maker], schema_membership: None } }
fn row() -> Violation {
    let allowed = Allowed::Finite { values: vec![json!("fixed")] };
    Violation { rule_id: "proof.subject".into(), role: Role::Maker,
        names: vec![Name { kind: NameKind::Field, value: "/value".into() }],
        bad_value: json!("bad"), allowed: allowed.clone(),
        repair: Repair::Edit { pointer: "/value".to_owned().try_into().unwrap(), op: EditOp::Replace, allowed },
        gating: false, evidence: vec![], evidence_truncated: 0 }
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
fn evidence_bounds_are_explicit_durable_and_utf8_safe() {
    let mut row = row();
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
    let mut row = row();
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
    let mut row = row();
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
fn source_instruction_wire_boundary_counts_utf8_bytes() {
    for instruction in [
        "a".repeat(INSTRUCTION_BYTES),
        "é".repeat(INSTRUCTION_BYTES / 2),
    ] {
        let repair: Repair = serde_json::from_value(
            json!({"kind":"source_edit", "path":"src/lib.rs", "instruction":instruction}),
        )
        .unwrap();
        let mut row = row();
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
    let mut row = row();
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
        let mut row = row();
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

#[test]
fn independent_repairs_and_evidence_never_get_clipped_to_fit() {
    let row = row();
    let single = render_all(std::slice::from_ref(&row), Role::Maker, RenderContext::Reply, &policy()).unwrap();
    let mut exact = row.clone();
    exact.repair = Repair::CantMeet { reason: "r".repeat(MESSAGE_BYTES) };
    assert!(render_all(&[exact], Role::Maker, RenderContext::Reply, &policy()).is_err());
    let count = MESSAGE_BYTES / (single.len() + 1) + 1;
    assert!(render_all(&vec![row; count], Role::Maker, RenderContext::Reply, &policy()).is_err());
}

#[test]
fn every_role_is_checked_and_tool_context_cannot_edit_a_reply() {
    for role in Role::ALL {
        let mut row = row();
        row.role = role;
        let reply = render_all(std::slice::from_ref(&row), role, RenderContext::Reply, &policy()).unwrap();
        assert!(reply.contains("replace reply field \"/value\""));
        assert!(render_all(std::slice::from_ref(&row), role, RenderContext::Tool, &policy()).is_err());
        let wrong_role = if role == Role::Maker { Role::Planner } else { Role::Maker };
        assert!(render_all(&[row], wrong_role, RenderContext::Reply, &policy()).is_err());
    }
}

#[test]
fn exact_field_permission_preserves_unmentioned_fields_and_propagates_schema_errors() {
    let mut row = row();
    let before = json!({"value":"bad", "kept":1});
    assert!(permits(std::slice::from_ref(&row), &before, &json!({"value":"fixed", "kept":1}), Role::Maker, &policy()).unwrap());
    assert!(!permits(std::slice::from_ref(&row), &before, &json!({"value":"fixed", "kept":2}), Role::Maker, &policy()).unwrap());
    let allowed = Allowed::Schema { keyword: SchemaKeyword::Pattern, value: json!("fixed") };
    row.allowed = allowed.clone();
    row.repair = Repair::Edit { pointer: "/value".to_owned().try_into().unwrap(), op: EditOp::Replace, allowed };
    assert!(permits(&[row], &before, &json!({"value":"fixed", "kept":1}), Role::Maker, &policy()).is_err());
}
