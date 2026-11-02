use crate::{
    Controller, Repository, Result, adapters::workers::protocol::relative_file, put, request,
};
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::LazyLock};
use tl3_kernel::{NodeId, Reply};
use tl3_store::{Commit, Snapshot};

impl Controller {
    /// Use the same effective policy as worker planning and scope trimming.
    pub fn set_request_protection(&mut self, config: &tl3_config::Config) {
        self.request_protection = config
            .github
            .repos
            .iter()
            .map(|repo| {
                (
                    Repository {
                        owner: repo.owner.clone(),
                        name: repo.name.clone(),
                        installation: repo.installation_id,
                    },
                    tl3_config::protected_paths(
                        &config.planning.protected_paths,
                        &repo.protected_paths,
                    ),
                )
            })
            .collect();
    }

    pub(crate) fn protected_reply(
        &mut self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        role: &str,
        reply: &Value,
    ) -> Result<Option<Reply>> {
        if !matches!(role, "ingest" | "audit" | "planner") {
            return Ok(None);
        }
        let repository = request(snapshot, node)?.repo;
        let defaults = tl3_config::Planning::default().protected_paths;
        let patterns = self
            .request_protection
            .iter()
            .find(|(repo, _)| *repo == repository)
            .map(|(_, paths)| paths.as_slice())
            .unwrap_or(&defaults);
        let mut paths = BTreeSet::new();
        if matches!(role, "ingest" | "audit") {
            let materials = self
                .store
                .worker_materials(&node.0.to_string())?
                .unwrap_or(json!({}));
            paths.extend(named_edits(
                materials["issue"]["body"].as_str().unwrap_or(""),
                patterns,
            ));
        }
        // This field means these edits are necessary for the goals, not merely
        // files the worker happened to inspect. A planner leaf is never a refusal.
        if (role != "planner" || reply["kind"] == "cant_meet")
            && let Some(claimed) = reply["protected_paths"].as_array()
            && !claimed.is_empty()
            && claimed
                .iter()
                .all(|path| path.as_str().is_some_and(|p| protected(p, patterns)))
        {
            paths.extend(claimed.iter().map(|p| p.as_str().unwrap().to_owned()));
        }
        if paths.is_empty() {
            return Ok(None);
        }
        commit.rows.push(put(
            "close_reasons",
            node.0.to_string(),
            json!({
                "reason":"protected_paths", "paths":paths, "source":role,
            }),
        ));
        Ok(Some(Reply::ProtectedPaths))
    }
}

fn protected(path: &str, patterns: &[String]) -> bool {
    relative_file(path.trim_end_matches('/')) && tl3_config::path_is_protected(path, patterns)
}

// Only these sections declare requested edits. Other issue prose is evidence,
// and may describe a protected file without asking TL3 to change it.
fn named_edits(body: &str, patterns: &[String]) -> BTreeSet<String> {
    static PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[A-Za-z0-9_./-]+(?:/[A-Za-z0-9_./-]+|\.[A-Za-z0-9_-]+)").unwrap()
    });
    static EDIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(add|change|edit|update|modify|delete|remove|create|write|implement|extend|regenerate|replace|rename|move|fix)\b").unwrap()
    });
    static NEGATED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(no|not|never|without|avoid|exclude|unchanged|unmodified|untouched|read.only|out.of.scope|possibly|maybe|optional|if.needed)\b|n't\b|n’t\b").unwrap()
    });
    static TARGET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(add|change|edit|update|modify|delete|remove|create|write|implement|extend|regenerate|replace|rename|move|fix)(?:\s+(?:the|file|directory))*\s+`?$").unwrap()
    });
    static LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(in|into|at)\s+`?$").unwrap());
    static LEADING_LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(?:[-*]|[0-9]+\.)\s+in\s+`?$").unwrap());
    let mut footprint = false;
    let mut goals = false;
    let mut paths = BTreeSet::new();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            let heading = line
                .trim_start_matches('#')
                .trim()
                .trim_end_matches(':')
                .to_ascii_lowercase();
            footprint = matches!(
                heading.as_str(),
                "expected footprint" | "footprint" | "files to change"
            );
            goals = matches!(
                heading.as_str(),
                "goals"
                    | "goal"
                    | "behaviour wanted"
                    | "behavior wanted"
                    | "requirements"
                    | "acceptance criteria"
            );
            continue;
        }
        let listed = line.starts_with("- ")
            || line.starts_with("* ")
            || line
                .split_once('.')
                .is_some_and(|(n, rest)| n.parse::<usize>().is_ok() && rest.starts_with(' '));
        if !listed || NEGATED.is_match(line) || !(footprint || goals && EDIT.is_match(line)) {
            continue;
        }
        let mut previous_path_end = 0;
        for found in PATH.find_iter(line) {
            if goals {
                let before = &line[..found.start()];
                let after = &line[found.end()..];
                let clause = before[previous_path_end..]
                    .rsplit([',', ';'])
                    .next()
                    .unwrap();
                previous_path_end = found.end();
                // Associate edit intent with this target. "Read X and edit Y"
                // and "add tests for X" do not request edits to X.
                if !(TARGET.is_match(before)
                    || LOCATION.is_match(before) && EDIT.is_match(clause)
                    || LEADING_LOCATION.is_match(before) && EDIT.is_match(after))
                {
                    continue;
                }
            }
            let path = found
                .as_str()
                .trim_end_matches('.')
                .strip_prefix("./")
                .unwrap_or(found.as_str().trim_end_matches('.'));
            if protected(path, patterns) {
                paths.insert(path.to_owned());
            }
        }
    }
    paths
}

pub(crate) fn closing_summary(
    controller: &Controller,
    snapshot: &Snapshot,
    commit: &mut Commit,
    delivery: &mut crate::executor::Delivery,
) -> Result<()> {
    let id = delivery.node().0.to_string();
    let reason = snapshot
        .rows
        .iter()
        .find(|r| r.table == "close_reasons" && r.id == id)
        .map(|r| r.value.clone())
        .or_else(|| {
            commit.rows.iter().find_map(|r| match r {
                tl3_store::RowChange::Put {
                    table,
                    id: row,
                    value,
                } if table == "close_reasons" && row == &id => Some(value.clone()),
                _ => None,
            })
        });
    let Some(reason) = reason.filter(|r| r["reason"] == "protected_paths") else {
        return Ok(());
    };
    let paths: Vec<String> = serde_json::from_value(reason["paths"].clone())?;
    let paths = paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    delivery.steering = Some(crate::steering::Task::ConsoleNote {
        text: format!(
            "TL3 cannot complete this request because it requires changes to protected paths: {paths}. This work belongs outside TL3. The issue stays open for the repository owner to handle, or narrow the request and comment `restart`."
        ),
    });
    if let Some(operations) = &controller.bootstrap.operations {
        let mut note = delivery.clone();
        note.key = format!("{}:protected-paths-ops", delivery.key);
        note.output = crate::executor::Output::Comment {
            node: delivery.node(),
            kind: crate::executor::CommentKind::ConsoleNote,
        };
        note.steering = Some(crate::steering::Task::ConsoleNote {
            text: format!(
                "https://github.com/{}/{}/issues/{} is not actionable by TL3 (protected_paths: {paths}); route this work outside TL3. The issue stays open.",
                delivery.repository.owner, delivery.repository.name, delivery.issue,
            ),
        });
        note.repository = Repository {
            owner: operations.owner.clone(),
            name: operations.name.clone(),
            installation: operations.installation_id,
        };
        note.issue = operations.number;
        note.pull_request = None;
        commit.rows.push(controller.allowance_row(note.clone()));
        commit.effects.push(note.into_effect());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_edits_use_all_effective_patterns_and_skip_context() {
        let patterns = tl3_config::protected_paths(&["global/**".into()], &["tools/ci/**".into()]);
        for (body, expected) in [
            (
                "## Goals\n- Edit `tools/ci/bazel_gate.py`.",
                vec!["tools/ci/bazel_gate.py"],
            ),
            (
                "## Expected footprint\n- `global/policy.txt`\n- `.github/workflows/ci.yml`\n- `conftest.py`",
                vec![
                    ".github/workflows/ci.yml",
                    "conftest.py",
                    "global/policy.txt",
                ],
            ),
            (
                "## Goals\n- Add tests in `tests/test_gate.py`, referring to `tools/ci/bazel_gate.py`.",
                vec![],
            ),
            (
                "## Goals\n- Add support without changing `.github/`.",
                vec![],
            ),
            ("## Goals\n- This doesn't touch `.github/`.", vec![]),
            ("## Goals\n- Read `tools/ci/bazel_gate.py`.", vec![]),
            (
                "## Goals\n- Read `tools/ci/bazel_gate.py` and update `docs/bazel.md`.",
                vec![],
            ),
            (
                "## Goals\n- Add tests for `tools/ci/bazel_gate.py`.",
                vec![],
            ),
            (
                "## Expected footprint\n- Possibly `tools/ci/bazel_gate.py`.",
                vec![],
            ),
            (
                "## Goals\n- In `tools/ci/bazel_gate.py`, add budget reruns.",
                vec!["tools/ci/bazel_gate.py"],
            ),
            (
                "## Expected footprint\n- `tests/test_gate.py`\n## Out of scope\n- Edit `tools/ci/bazel_gate.py`.",
                vec![],
            ),
            ("## Goals\n- Edit `../tools/ci/bazel_gate.py`.", vec![]),
        ] {
            assert_eq!(
                named_edits(body, &patterns).into_iter().collect::<Vec<_>>(),
                expected,
                "{body}"
            );
        }
    }
}

pub(crate) fn ingest_preflight(inputs: &Value) -> Option<Value> {
    let patterns: Vec<String> = inputs["protected_paths"]
        .as_array()?
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect();
    let paths = named_edits(inputs["issue"]["body"].as_str()?, &patterns);
    (!paths.is_empty()).then(|| {
        json!({"kind":"early_end", "outcome":"not_actionable",
        "reason":"protected_paths", "protected_paths":paths})
    })
}
