//! Canonical maker footprint (#894, PRD #878 F1).

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeSet, ops::Deref};

/// #894: only construction may canonicalise a plan; consumers share its result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Footprint(Vec<String>);

impl Footprint {
    pub fn new(paths: Vec<String>) -> Result<Self, &'static str> {
        let mut seen = BTreeSet::new();
        let mut canonical = Vec::new();
        for path in paths {
            if path.contains(['\r', '\n']) {
                return Err("Use a single-line relative path without carriage returns or newlines");
            }
            if path.is_empty() || path.starts_with('/') || path.contains(['\\', '\0']) {
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
            let path = parts.join("/");
            if seen.insert(path.clone()) {
                canonical.push(path);
            }
        }
        Ok(Self(canonical))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl TryFrom<Vec<String>> for Footprint {
    type Error = &'static str;

    fn try_from(paths: Vec<String>) -> Result<Self, Self::Error> {
        Self::new(paths)
    }
}

impl Deref for Footprint {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'de> Deserialize<'de> for Footprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let paths = Vec::<String>::deserialize(deserializer)?;
        let footprint = Self::new(paths.clone()).map_err(serde::de::Error::custom)?;
        if footprint.as_slice() != paths {
            return Err(serde::de::Error::custom(
                "footprint must already be canonical and deduplicated",
            ));
        }
        Ok(footprint)
    }
}

impl JsonSchema for Footprint {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Footprint".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "array", "uniqueItems": true,
            "items": {
                "type": "string", "minLength": 1,
                "pattern": r"^[^/\\\u0000]+(/[^/\\\u0000]+)*$",
                "not": {"pattern": r"(^|/)(\.|\.\.|\.git)(/|$)"}
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #894 SF8: shared examples pin spelling, safety, Unicode and case at the language seam.
    #[test]
    fn shared_python_path_vectors() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../workers/tests/fixtures/footprint-paths.json"
        ))
        .unwrap();
        for case in cases {
            let result = Footprint::new(vec![case["path"].as_str().unwrap().into()]);
            if let Some(expected) = case["canonical"].as_str() {
                assert_eq!(result.unwrap().as_slice(), [expected], "{case}");
            } else {
                assert!(result.is_err(), "{case}");
            }
        }
    }

    #[test]
    fn canonicalises_and_deduplicates_declared_paths() {
        let footprint = Footprint::new(
            ["./x.py", "a//b.py", "dir/", "a/./b.py", "x.py"]
                .map(String::from)
                .to_vec(),
        )
        .unwrap();
        assert_eq!(footprint.as_slice(), ["x.py", "a/b.py", "dir"]);
    }

    #[test]
    fn footprint_limits_are_utf8_bytes_and_reject_line_breaks() {
        let full = vec!["x".repeat(255); 12].join("/") + "/x";
        assert_eq!(full.len(), 3073);
        for path in [
            "x\n.py".into(),
            "x\r.py".into(),
            "é".repeat(128),
            full.clone(),
        ] {
            assert!(
                Footprint::new(vec![path.clone()]).is_err(),
                "accepted {path:?}"
            );
        }
        for path in ["x".repeat(255), "é".repeat(127), full[..3072].into()] {
            assert!(
                Footprint::new(vec![path.clone()]).is_ok(),
                "rejected {path:?}"
            );
        }
    }

    #[test]
    fn refuses_paths_that_cannot_name_repository_files() {
        for path in [
            "..",
            "a/../b.py",
            "/x.py",
            "//x.py",
            "",
            ".",
            "./",
            "x\0.py",
            "C:\\x.py",
            ".git/config",
        ] {
            assert!(
                Footprint::new(vec![path.into()]).is_err(),
                "accepted {path:?}"
            );
        }
    }

    #[test]
    fn maker_input_exposes_a_required_canonical_footprint() {
        let schema = crate::roles::maker_input::schema();
        assert_eq!(schema["properties"]["footprint"]["type"], "array");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("footprint"))
        );
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn canonical_wire_round_trip_and_refusal_of_noncanonical_wire_values() {
        let footprint = Footprint::new(vec!["./x.py".into(), "a//b.py".into()]).unwrap();
        let wire = serde_json::to_value(&footprint).unwrap();
        assert_eq!(wire, serde_json::json!(["x.py", "a/b.py"]));
        assert_eq!(
            serde_json::from_value::<Footprint>(wire).unwrap(),
            footprint
        );
        for paths in [
            vec!["./x.py"],
            vec!["a//b.py"],
            vec!["x.py", "x.py"],
            vec!["../x.py"],
            vec!["/x.py"],
        ] {
            assert!(serde_json::from_value::<Footprint>(serde_json::json!(paths)).is_err());
        }
    }
}
