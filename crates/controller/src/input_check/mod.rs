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
struct Check {
    goal_ids: Vec<String>,
}
#[derive(Deserialize)]
struct Plan {
    files: Vec<String>,
    checks: Vec<Check>,
}
#[derive(Deserialize)]
struct Maker {
    goals: Vec<Goal>,
    plan: Plan,
    #[serde(default)]
    feedback: Vec<Check>,
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
    } else if inputs
        .plan
        .checks
        .iter()
        .chain(&inputs.feedback)
        .any(|check| check.goal_ids.iter().any(|id| !goals.contains(id.as_str())))
    {
        Some("unknown goal id")
    } else {
        let covered: BTreeSet<_> = inputs
            .plan
            .checks
            .iter()
            .flat_map(|check| check.goal_ids.iter())
            .collect();
        inputs
            .goals
            .iter()
            .any(|goal| goal.stage.as_deref() != Some("publishing") && !covered.contains(&goal.id))
            .then_some("checks must cover every maker and checking goal")
    };
    Ok(violation.into_iter().map(str::to_owned).collect())
}

fn declared_file(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.contains(['\\', '\0']) || path.starts_with('/') {
        return Err("Use a relative worktree path");
    }
    if path.split('/').any(|part| part == "..") {
        return Err("Parent traversal is forbidden");
    }
    let parts: Vec<_> = path
        .split('/')
        .filter(|part| !matches!(*part, "" | "."))
        .collect();
    if parts.contains(&".git") {
        return Err("Git metadata paths are forbidden");
    }
    if path.len() > 3072 || parts.iter().any(|part| part.len() > 255) {
        return Err("Path is too long");
    }
    if parts.is_empty() {
        return Err("expected a declared file");
    }
    Ok(())
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
    fn maker_matches_python_reference() {
        let script = r#"
import json, sys
from pathlib import Path
sys.path.insert(0, str(Path(sys.argv[1]) / 'workers/tests/p5'))
from maker_reference import _check_inputs
rows = []
base = {'goals': [{'id': 'a'}, {'id': 'b', 'stage': 'checking'}, {'id': 'c', 'stage': 'publishing'}],
        'plan': {'files': ['src/a.rs'], 'checks': [{'goal_ids': ['a', 'b']}]}}
from copy import deepcopy
for kind in ['valid', 'duplicate', 'unknown', 'feedback', 'uncovered', 'dot', 'parent', 'git', 'absolute', 'long', 'unicode']:
    value = deepcopy(base)
    if kind == 'duplicate': value['goals'].append(value['goals'][0])
    if kind == 'unknown': value['plan']['checks'][0]['goal_ids'] = ['unknown']
    if kind == 'feedback': value['feedback'] = [{'goal_ids': ['unknown']}]
    if kind == 'uncovered': value['plan']['checks'][0]['goal_ids'] = ['a']
    paths = {'dot': '.', 'parent': 'x/../y', 'git': 'x/.git/HEAD', 'absolute': '/x', 'long': 'x'*256, 'unicode': 'é'*128}
    if kind in paths: value['plan']['files'] = [paths[kind]]
    try:
        _check_inputs(value)
        expected = []
    except ValueError as error: expected = [str(error)]
    rows.append({'inputs': value, 'expected': expected})
print(json.dumps(rows))
"#;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
            .args(["-c", script])
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert!(rows.len() >= 10);
        for row in rows {
            assert_eq!(
                json!(maker(&row["inputs"]).unwrap()),
                row["expected"],
                "{row}"
            );
        }
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
    #[test]
    fn maker_extraction_preserves_real_runtime_schedules() {
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
}
