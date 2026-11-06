//! Preserve the published shape separately from the ordered byte renderer.
use serde_json::Value;
use tl3_contracts::roles;
fn differences(a: &Value, b: &Value, path: &str, found: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a
                .keys()
                .chain(b.keys())
                .collect::<std::collections::BTreeSet<_>>()
            {
                match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => differences(a, b, &format!("{path}/{key}"), found),
                    (a, b) => found.push(format!("{path}/{key}: generated {a:?}, published {b:?}")),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                differences(a, b, &format!("{path}/{i}"), found);
            }
        }
        _ if a != b => found.push(format!("{path}: generated {a}, published {b}")),
        _ => (),
    }
}
macro_rules! shape {
    ($test:ident,$module:ident,$name:literal) => {
        #[test]
        fn $test() {
            let a = roles::$module::schema_value();
            let b: Value = serde_json::from_str(include_str!(concat!(
                "../../../workers/src/tl3_workers/schemas/",
                $name,
                "-reply.json"
            )))
            .unwrap();
            let mut found = vec![];
            differences(&a, &b, "", &mut found);
            assert!(found.is_empty(), "{}", found.join("\n"));
        }
    };
}
shape!(audit, audit_reply, "audit");
shape!(planner, planner_reply, "planner");
shape!(reviewer, reviewer_reply, "reviewer");
shape!(postmortem, postmortem_reply, "postmortem");
