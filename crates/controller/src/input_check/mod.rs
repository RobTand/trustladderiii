//! P5: semantic maker inputs and schema checks at their original worker boundaries.
use serde_json::Value;

mod schema;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
struct Goal {
    id: String,
    #[serde(default)]
    stage: Option<String>,
}
#[derive(Deserialize)]
struct FeedbackGoalReferences {
    goal_ids: Vec<String>,
}
#[derive(Deserialize)]
struct Plan {
    files: Vec<String>,
    checks: Vec<tl3_contracts::Check>,
}
#[derive(Deserialize)]
struct Maker {
    goals: Vec<Goal>,
    plan: Plan,
    #[serde(default)]
    feedback: Vec<FeedbackGoalReferences>,
}

pub fn maker(inputs: &Value) -> crate::Result<Vec<String>> {
    let inputs: Maker = serde_json::from_value(inputs.clone())?;
    let goals: BTreeSet<_> = inputs.goals.iter().map(|goal| goal.id.as_str()).collect();
    let violation = if goals.len() != inputs.goals.len() {
        Some("goal ids must be unique")
    } else if let Some(message) = inputs
        .plan
        .files
        .iter()
        .find_map(|path| declared_file(path).err())
    {
        Some(message)
    } else if inputs.feedback.iter().any(|item| item.goal_ids.iter().any(|id| !goals.contains(id.as_str()))) {
        Some("unknown feedback goal id")
    } else {
        None
    };
    if let Some(violation) = violation { return Ok(vec![violation.into()]); }
    let required: Vec<_> = inputs.goals.iter().enumerate()
        .filter(|(_, goal)| goal.stage.as_deref() != Some("publishing"))
        .map(|(index, _)| tl3_contracts::GoalId(index as u64)).collect();
    if let Err(error) = tl3_contracts::checks::admit_plan(&inputs.plan.checks, inputs.goals.len(), &required) {
        return Ok(vec![error]);
    }
    for check in &inputs.plan.checks {
        for test in &check.tests {
            if tl3_harness_core::node_id::NodeId::parse(test, tl3_harness_core::node_id::Mode::Exact).is_err() {
                return Ok(vec!["checks need exact pytest selectors".into()]);
            }
        }
    }
    Ok(vec![])
}

fn declared_file(path: &str) -> Result<(), &'static str> {
    tl3_contracts::footprint::Footprint::new(vec![path.into()]).map(|_| ())
}

pub fn request(request: &Value) -> crate::Result<Vec<String>> {
    if !schema::valid("request", request)? {
        return Ok(vec!["worker request breaks schema".into()]);
    }
    let Some(
        role @ ("ingest" | "audit" | "planner" | "maker" | "reviewer" | "classifier" | "writer"
        | "postmortem"),
    ) = request["role"].as_str()
    else {
        return Ok(vec!["unsupported worker role".into()]);
    };
    Ok(
        if schema::valid(&format!("{role}-input"), &request["inputs"])? {
            vec![]
        } else {
            vec!["worker inputs break role schema".into()]
        },
    )
}

/// Standalone worker entrypoint checks stay before chdir and all runtime writes.
pub fn check(role: Option<&str>) -> crate::Result<()> {
    use std::io::{Read, Write};
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    let value = crate::adapters::workers::json::parse(&bytes)?;
    let violations = match role {
        Some("maker") => maker(&value)?,
        Some("request") => request(&value)?,
        _ => {
            return Err(crate::Error::Invalid(
                "usage: tl3 check-inputs <request|maker>",
            ));
        }
    };
    serde_json::to_writer(
        std::io::stdout().lock(),
        &serde_json::json!({"violations": violations}),
    )?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn footprint_shared_vectors_at_maker_input() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../../workers/tests/fixtures/footprint-paths.json"
        ))
        .unwrap();
        for case in cases {
            assert_eq!(
                declared_file(case["path"].as_str().unwrap()).is_ok(),
                case["canonical"].is_string(),
                "{case}"
            );
        }
        assert!(declared_file("src/bad\n.py").is_err());
        assert!(declared_file("src/bad\r.py").is_err());
    }

    #[test]
    fn maker_preserves_numeric_original_indices_and_distinct_feedback_ids() {
        let base = json!({"goals":[{"id":"a"},{"id":"b","stage":"checking"},{"id":"c","stage":"publishing"}],
            "plan":{"files":["src/a.rs"],"checks":[{"name":"behavior","kind":"demonstrating",
                "tests":["tests/test_a.py::test_a"],"goal_ids":[0,1]}]}, "feedback":[{"goal_ids":["b"]}]});
        assert!(maker(&base).unwrap().is_empty());
        for (path, replacement) in [
            ("/goals/1/id",json!("a")), ("/feedback/0/goal_ids",json!(["unknown"])),
            ("/plan/checks/0/goal_ids",json!([0])), ("/plan/checks/0/goal_ids",json!([0,3])),
            ("/plan/checks/0/goal_ids",json!([])), ("/plan/checks/0/name",json!(" ")),
            ("/plan/checks/0/tests",json!(["tests/test_a.py"])),
        ] {
            let mut candidate=base.clone(); *candidate.pointer_mut(path).unwrap()=replacement;
            assert!(!maker(&candidate).unwrap().is_empty(), "{candidate}");
        }
        for path in [".","x/../y","x/.git/HEAD","/x",&"x".repeat(256),&"é".repeat(128)] {
            let mut candidate=base.clone(); candidate["plan"]["files"]=json!([path]);
            assert!(!maker(&candidate).unwrap().is_empty());
        }
        let mut omitted=base.clone(); omitted["plan"]["checks"][0].as_object_mut().unwrap().remove("goal_ids");
        assert!(maker(&omitted).is_err());
        let mut strings=base.clone(); strings["plan"]["checks"][0]["goal_ids"]=json!(["a","b"]);
        assert!(maker(&strings).is_err());
    }

    #[test]
    fn invalid_envelope_stays_at_the_worker_boundary() {
        assert_eq!(
            request(&json!({})).unwrap(),
            vec!["worker request breaks schema"]
        );
    }
}

#[cfg(test)]
mod io_schedules {
    use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
    fn maker_schedules(kind: &str) {
        let mut runner = TestRunner::deterministic();
        let strategy = (0u8..3, any::<bool>());
        let mut rows = Vec::new();
        for _ in 0..24 {
            let (crash, read_failure) = strategy.new_tree(&mut runner).unwrap().current();
            rows.push(serde_json::json!({"crash": crash, "read_failure": read_failure}));
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        use std::io::Write;
        let mut child = std::process::Command::new(root.join("workers/.venv/bin/python"))
            .arg(root.join("workers/tests/p5/schedules.py"))
            .arg(kind)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&rows).unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("persisted bytes"));
    }
    // Each violation keeps all read schedules and every real writer crash cut.
    macro_rules! schedules {
        ($($name:ident => $kind:literal),* $(,)?) => {
            $(#[test] fn $name() { maker_schedules($kind); })*
        };
    }
    schedules!(
        maker_duplicate_schedules => "duplicate",
        maker_empty_schedules => "empty",
        maker_parent_schedules => "parent",
        maker_git_schedules => "git",
        maker_absolute_schedules => "absolute",
        maker_long_schedules => "long",
        maker_unknown_schedules => "unknown",
        maker_feedback_schedules => "feedback",
        maker_coverage_schedules => "maker-coverage",
        checking_coverage_schedules => "checking-coverage",
    );
}
