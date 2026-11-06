//! #884/#929: exercise resolved refusal rows through their public interface.
use serde_json::{Value, json};
use tl3_contracts::rules::*;

fn terminal(value: &Value) -> bool { value.as_object().is_some_and(|v| v.get("kind") == Some(&json!("cant_meet"))) }
const TERMINALS: &[TerminalForm] = &[TerminalForm { role: Role::Maker, predicate: terminal, phrase: "Return kind=cant_meet." }, TerminalForm { role: Role::Planner, predicate: terminal, phrase: "Return kind=cant_meet." }];
fn policy() -> Policy<'static> { Policy { terminals: TERMINALS, source_editors: &[Role::Maker] } }
fn allowed() -> Allowed { Allowed::Finite { values: vec![json!("good")] } }
fn row() -> Rule<Value> {
    Rule { id: "reply.value", check: |v| if v["value"] == "good" { vec![] } else { vec![v.clone()] }, names: |_| vec![Name { kind: NameKind::Field, value: "/value".into() }], bad_value: |v| v["value"].clone(), allowed: Callback::Facts(|_| allowed()), repair: Callback::Role(|_, role| if role == Role::Maker { Repair::Edit { pointer: "/value".to_owned().try_into().unwrap(), op: EditOp::Replace, allowed: allowed() } } else { Repair::CantMeet { reason: "no permitted edit".into() } }), gating: false, roles: vec![Role::Maker, Role::Planner] }
}
fn registered() -> Rule<Value> {
    row().register(&Registration { example: json!({"value":"bad"}), reply: json!({"value":"bad"}), witness: Witness::ByRole([(Role::Maker, json!({"value":"good"})), (Role::Planner, json!({"kind":"cant_meet"}))].into()) }, &policy()).unwrap()
}

#[test]
fn callbacks_resolve_per_role_and_preserve_wire_identity() {
    let rule = registered();
    for role in [Role::Maker, Role::Planner] {
        let rows = rule.evaluate(&json!({"value":"bad"}), role, &policy()).unwrap();
        assert_eq!(rows.len(), 1);
        let encoded = serde_json::to_value(&rows).unwrap();
        assert_eq!(encoded[0]["role"], role.as_str());
        assert!(encoded[0].get("evidence").is_none());
        assert_eq!(serde_json::from_value::<Vec<Violation>>(encoded).unwrap(), rows);
        let rendered = render_all(&rows, role, RenderContext::Reply, &policy()).unwrap();
        assert!(rendered.contains("/value"));
        assert!(rendered.contains("good"));
        if role == Role::Maker {
            assert!(permits(&rows, &json!({"value":"bad"}), &json!({"value":"good"}), role, &policy()).unwrap());
            assert!(!permits(&rows, &json!({"value":"bad","other":1}), &json!({"value":"good","other":2}), role, &policy()).unwrap());
            assert!(render_all(&rows, role, RenderContext::Tool, &policy()).is_err());
        } else {
            assert!(!render_all(&rows, role, RenderContext::Tool, &policy()).unwrap().contains("Return"));
        }
    }
}

#[test]
fn explicit_witness_mapping_checks_each_mixed_repair_role() {
    let mut rule = row();
    rule.repair = Callback::Role(|_, role| if role == Role::Maker { Repair::SourceEdit { path: "src/lib.rs".into(), instruction: "Fix the assertion".into() } } else { Repair::CantMeet { reason: "source tools unavailable".into() } });
    rule.names = |_| vec![Name { kind: NameKind::Path, value: "src/lib.rs".into() }];
    let fixture = Registration { example: json!({"value":"bad"}), reply: json!({"value":"bad"}), witness: Witness::ByRole([(Role::Maker, Value::Null), (Role::Planner, json!({"kind":"cant_meet"}))].into()) };
    let rule = rule.register(&fixture, &policy()).unwrap();
    let rows = rule.evaluate(&fixture.example, Role::Maker, &policy()).unwrap();
    assert_eq!(render_all(&rows, Role::Maker, RenderContext::Reply, &policy()).unwrap(), render_all(&rows, Role::Maker, RenderContext::Tool, &policy()).unwrap());
    assert!(permits(&rows, &fixture.reply, &fixture.reply, Role::Maker, &policy()).unwrap());
    assert!(!permits(&rows, &fixture.reply, &json!({"value":"good"}), Role::Maker, &policy()).unwrap());
    assert!(rule.evaluate(&fixture.example, Role::Audit, &policy()).is_err());
}

#[test]
fn invalid_context_is_rejected_even_without_violations() {
    assert!(serde_json::from_value::<RenderContext>(json!("unknown")).is_err());
    assert_eq!(RenderContext::default(), RenderContext::Reply);
    assert_eq!(render_all(&[], Role::Maker, RenderContext::Reply, &policy()).unwrap(), "");
}

#[test]
fn field_pointer_parsing_preserves_escaped_generated_segments() {
    for count in 1..80 {
        let value = format!("/checks/{count}/{}", "~0~1".repeat(count));
        let pointer: FieldPointer = serde_json::from_value(json!(value)).unwrap();
        assert_eq!(pointer.as_str(), value);
        for suffix in ["~", "~2", "~x", "\n"] { assert!(serde_json::from_value::<FieldPointer>(json!(format!("{value}{suffix}"))).is_err()); }
    }
    for invalid in ["", "relative", "#/value"] { assert!(serde_json::from_value::<FieldPointer>(json!(invalid)).is_err()); }
}

#[test]
fn remaining_role_roots_join_generated_inventory_without_changing_bytes() {
    let generated = tl3_contracts::generated();
    for (role, committed) in [
        ("audit", include_str!("../../../workers/src/tl3_workers/schemas/audit-reply.json")),
        ("planner", include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json")),
        ("reviewer", include_str!("../../../workers/src/tl3_workers/schemas/reviewer-reply.json")),
        ("postmortem", include_str!("../../../workers/src/tl3_workers/schemas/postmortem-reply.json")),
    ] {
        let file = generated.iter().find(|file| file.path.ends_with(&format!("/{role}-reply.json"))).unwrap_or_else(|| panic!("missing Rust-owned {role} reply"));
        assert_eq!(file.text, committed, "{role} schema bytes and prompt identity");
    }
}
