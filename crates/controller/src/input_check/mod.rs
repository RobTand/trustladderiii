//! P5: semantic maker inputs and schema checks at their original worker boundaries.
use serde_json::Value;

pub fn maker(_inputs: &Value) -> crate::Result<Vec<String>> {
    Ok(vec!["P5 maker stub".into()])
}

pub fn request(_request: &Value) -> crate::Result<Vec<String>> {
    Ok(vec!["P5 request stub".into()])
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
