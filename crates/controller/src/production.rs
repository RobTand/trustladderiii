use crate::{Result, adapters, executor::Registry, ingress::Webhooks};

pub struct Registration {
    pub adapters: Registry,
    pub webhooks: Webhooks,
    pub workers: adapters::workers::WorkerOutputSource,
    pub bell: adapters::prismabuild::Source,
    pub mergify: adapters::mergify::Translator,
}

pub fn register(_config: &tl3_config::Config) -> Result<Registration> {
    todo!("production registration")
}

pub(crate) fn maker_inputs(_brief: &serde_json::Value, _plan: &serde_json::Value) -> Result<serde_json::Value> {
    todo!("accepted production materials")
}

pub(crate) fn repository_materials(_repository: &std::path::Path, _commit: &str) -> Result<serde_json::Value> {
    todo!("committed repository materials")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, process::Command};

    #[test]
    fn accepted_plan_supplies_maker_goals_and_named_checks() {
        let brief = json!({"brief":"Return a greeting", "goals":[{"description":"Say hello","verification":"test"}]});
        let plan = json!({"kind":"leaf","files":["greeting.py"],"checks":[{"kind":"demonstrating","node_ids":["tests/test_greeting.py::test_hello"],"goal_ids":[0]}]});
        let inputs = maker_inputs(&brief, &plan).unwrap();
        assert_eq!(inputs["goals"][0]["id"], "goal-1");
        assert_eq!(inputs["plan"]["checks"][0]["goal_ids"], json!(["goal-1"]));
        assert_eq!(inputs["plan"]["checks"][0]["tests"], json!(["tests/test_greeting.py::test_hello"]));
        let invalid = json!({"kind":"leaf","files":[],"checks":[{"kind":"regression","node_ids":["test.py::test_it"],"goal_ids":[9]}]});
        assert!(maker_inputs(&brief, &invalid).is_err());
    }

    #[test]
    fn repository_materials_use_committed_text_without_following_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git").current_dir(root.path()).args(["-c","user.name=Fixture","-c","user.email=fixture@example.invalid","-c","commit.gpgsign=false","-c","core.hooksPath=/dev/null"]).args(args).output().unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--quiet"]);
        fs::write(root.path().join("README.md"), "Committed readme").unwrap();
        fs::write(root.path().join("greeting.py"), "hello\n").unwrap();
        fs::write(root.path().join("binary"), [0, 255]).unwrap();
        std::os::unix::fs::symlink("/does/not/exist", root.path().join("link")).unwrap();
        git(&["add", "."]); git(&["commit", "--quiet", "-m", "Fixture"]);
        let sha = git(&["rev-parse", "HEAD"]);
        fs::write(root.path().join("greeting.py"), "dirty\n").unwrap();
        let repo = repository_materials(root.path(), &sha).unwrap();
        assert_eq!(repo["readme"], "Committed readme");
        assert!(repo["materials"].as_array().unwrap().iter().any(|m| m["path"]=="greeting.py" && m["content"]=="hello\n"));
        assert!(!repo["materials"].as_array().unwrap().iter().any(|m| m["path"]=="link" || m["path"]=="binary"));
        assert_eq!(repo["file_list"].as_array().unwrap().len(), 4);
    }
}
