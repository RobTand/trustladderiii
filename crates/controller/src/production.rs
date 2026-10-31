use crate::{Result, adapters, executor::Registry, ingress::Webhooks};
use tl3_worktrees::{CommitId, Worktrees};

pub struct Registration {
    pub adapters: Registry,
    pub webhooks: Webhooks,
    pub workers: adapters::workers::WorkerOutputSource,
    pub bell: adapters::prismabuild::Source,
    pub mergify: adapters::mergify::Translator,
}

pub fn register(config: &tl3_config::Config) -> Result<Registration> {
    use crate::{Bootstrap, Error, executor::Upstream, ingress::CatchUp};
    use std::{collections::BTreeMap, fs, time::Duration};
    use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
    use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};
    use tl3_store::Store;

    config.validate()?;
    config.validate_secret_files()?;
    let bootstrap = Bootstrap::from_config(config)?;
    if bootstrap.operations.is_none() {
        return Err(Error::Invalid(
            "production requires [github.operations]; run deploy.py install",
        ));
    }
    let listen = config
        .github
        .webhook_listen
        .ok_or(Error::Invalid("production requires github.webhook_listen"))?;
    let bytes = fs::read(&config.github.app_credentials_file)?;
    let app: AppConfig = serde_json::from_slice(&bytes)?;
    let secret = app.webhook_secret.as_bytes().to_vec();
    let mut clients = BTreeMap::new();
    for repo in bootstrap.github_repositories() {
        if let std::collections::btree_map::Entry::Vacant(entry) = clients.entry(repo.installation)
        {
            entry.insert(GitHub::new(
                serde_json::from_slice(&bytes)?,
                repo.installation,
                ClientOptions::default(),
            )?);
        }
    }
    let mut store = Store::open(&config.store)?;
    let epoch = store.load()?.epoch.get();
    let records = config.workers.runs.join(".controller");
    fs::create_dir_all(&records)?;
    let mut adapters = Registry::default();
    let mut worktrees = adapters::worktrees::Real::new(&config.worktrees_root, epoch)?;
    for repo in &bootstrap.repos {
        let path = repository_path(&config.worktrees_root, repo);
        fs::create_dir_all(&path)?;
        if !path.join("HEAD").exists() {
            Worktrees::init_bare(&path)?;
        }
        worktrees.serve_github(repo.clone(), &path, clients[&repo.installation].clone())?;
    }
    let supervisor = adapters::workers::Supervisor::new(config, &records.join("workers"))?
        .with_materials(Materials {
            clients: clients.clone(),
            worktrees: config.worktrees_root.clone(),
        });
    let workers = supervisor.source();
    let tools = &config.prismabuild.tools_dir;
    let tool = Tool {
        program: "python3".into(),
        args: vec![tools.join("pbrun.py").into()],
    };
    let queue = tools
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or(Error::Invalid(
            "PrismaBuild tools need an installed fleet root",
        ))?
        .join("pb-queue");
    let client = PrismaBuild::new(Options {
        queue_root: queue,
        tool,
        bell_url: "http://127.0.0.1:9470/".into(),
    })
    .map_err(|e| Error::Adapter(Box::new(e)))?;
    let app_root = config
        .workers
        .python
        .ancestors()
        .nth(4)
        .ok_or(Error::Invalid(
            "workers.python needs the installed venv layout",
        ))?;
    let pb = adapters::prismabuild::Adapter::new(
        client,
        adapters::prismabuild::Settings {
            records: records.join("prismabuild"),
            harness: app_root.join("harness/src/tl3_harness"),
            python: vec![config.workers.python.clone().into_os_string()],
            image: config.prismabuild.container_image.clone(),
            demand: Demand {
                cpus: 1.try_into().expect("positive demand"),
                mem_gb: 2,
                gpu: false,
            },
            quiet: Duration::from_secs(u64::from(config.allowances.prismabuild.no_progress_secs)),
            priority: 0,
            result_tool: Tool {
                program: "python3".into(),
                args: vec![tools.join("pbwait.py").into()],
            },
        },
    )?;
    let bell = pb.source();
    let writer = adapters::workers::writer::ProcessWriter::new(config, &records.join("writer"))?;
    adapters.register(
        Upstream::GitHub,
        adapters::github::Adapter::new(
            clients.clone(),
            writer,
            epoch,
            Policy::default(),
            &records.join("github"),
            config.github.operations.clone(),
        )?,
    );
    adapters.register(Upstream::Worktrees, worktrees);
    adapters.register(Upstream::Workers, supervisor);
    adapters.register(Upstream::PrismaBuild, pb);
    adapters.register(
        Upstream::Mergify,
        adapters::mergify::Adapter::new(clients.clone(), epoch, &records.join("mergify"))?,
    );
    let mergify = adapters::mergify::Translator::new(clients.clone())?;
    let webhooks = Webhooks::bind(
        listen,
        Store::open(&config.store)?,
        bootstrap,
        secret,
        Some(CatchUp::new(clients)?),
    )?;
    Ok(Registration {
        adapters,
        webhooks,
        workers,
        bell,
        mergify,
    })
}

fn repository_path(root: &std::path::Path, repo: &crate::Repository) -> std::path::PathBuf {
    root.join(".repositories")
        .join(repo.owner.to_ascii_lowercase())
        .join(format!("{}.git", repo.name.to_ascii_lowercase()))
}

#[derive(Clone)]
pub(crate) struct Materials {
    clients: std::collections::BTreeMap<u64, tl3_github::GitHub>,
    worktrees: std::path::PathBuf,
}

impl Materials {
    pub(crate) fn load(&self, delivery: &crate::executor::Delivery) -> Result<serde_json::Value> {
        use serde_json::json;
        let client = &self.clients[&delivery.repository.installation];
        let runtime = crate::executor::runtime()?;
        let name = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let issue = runtime.block_on(client.issue(&name, delivery.issue))?;
        let comments = runtime.block_on(client.comments(&name, delivery.issue))?;
        let path = repository_path(&self.worktrees, &delivery.repository);
        let git = tl3_worktrees::Worktrees::open(&path, &self.worktrees)
            .map_err(|e| crate::Error::Adapter(Box::new(e)))?;
        let remote = tl3_worktrees::Remote::https(&format!("https://github.com/{name}.git"))
            .map_err(|e| crate::Error::Adapter(Box::new(e)))?;
        let token =
            tl3_worktrees::InstallationToken::new(runtime.block_on(client.installation_token())?)
                .map_err(|e| crate::Error::Adapter(Box::new(e)))?;
        let base = git
            .fetch_main(&remote, Some(&token))
            .map_err(|e| crate::Error::Adapter(Box::new(e)))?;
        let repo = repository_materials(&path, base.as_str())?;
        let issue = json!({"number": issue.number, "title":issue.title, "body":delivery.request_text.clone().or(issue.body).unwrap_or_default(), "author":"unknown",
            "comments":comments.iter().map(|c| json!({"author":c.user.login,"body":c.body})).collect::<Vec<_>>()});
        let inputs = json!({"issue":issue,"repo":repo});
        Ok(json!({"issue":issue,"repo":repo,"ingest":inputs,"audit":inputs}))
    }
}

pub(crate) fn maker_inputs(
    brief: &serde_json::Value,
    plan: &serde_json::Value,
) -> Result<serde_json::Value> {
    use crate::Error;
    use serde_json::{Value, json};
    if plan["kind"] != "leaf" {
        return Err(Error::Invalid("making requires an accepted leaf plan"));
    }
    let mut goals = brief["goals"]
        .as_array()
        .ok_or(Error::Invalid("accepted brief needs goals"))?
        .clone();
    for (index, goal) in goals.iter_mut().enumerate() {
        goal["id"] = json!(format!("goal-{}", index + 1));
    }
    let checks = plan["checks"].as_array().ok_or(Error::Invalid("accepted plan needs checks"))?.iter().enumerate().map(|(index, check)| {
        let ids = check["goal_ids"].as_array().ok_or(Error::Invalid("plan check needs goal indexes"))?.iter().map(|id| {
            id.as_u64().and_then(|id| goals.get(id as usize)).map(|g| g["id"].clone()).ok_or(Error::Invalid("plan check names an unknown goal"))
        }).collect::<Result<Vec<_>>>()?;
        Ok(json!({"name":format!("check-{}",index + 1), "kind":check["kind"], "tests":check["node_ids"], "goal_ids":ids}))
    }).collect::<Result<Vec<Value>>>()?;
    Ok(
        json!({"brief":brief["brief"], "goals":goals, "plan":{"files":plan["files"], "checks":checks}}),
    )
}

pub(crate) fn repository_materials(
    repository: &std::path::Path,
    commit: &str,
) -> Result<serde_json::Value> {
    use crate::Error;
    use serde_json::json;
    let commit = CommitId::new(commit)?;
    let repository = Worktrees::open(repository, repository)?;
    let tree = repository.list_tree(&commit)?;
    let mut files = Vec::new();
    let mut materials = Vec::new();
    let mut readme = String::new();
    let mut total = 0usize;
    for entry in tree.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = std::str::from_utf8(entry)
            .map_err(|_| Error::Invalid("repository paths must be UTF-8"))?;
        let (metadata, path) = entry
            .split_once('\t')
            .ok_or(Error::Invalid("invalid Git tree entry"))?;
        let fields: Vec<_> = metadata.split_whitespace().collect();
        if fields.len() != 4 {
            return Err(Error::Invalid("invalid Git tree metadata"));
        }
        files.push(path.to_owned());
        if files.len() > 10000 {
            return Err(Error::Invalid("repository snapshot exceeds the file bound"));
        }
        if !matches!(fields[0], "100644" | "100755") || fields[1] != "blob" {
            continue;
        }
        let size: usize = fields[3]
            .parse()
            .map_err(|_| Error::Invalid("invalid Git blob size"))?;
        if size > 1024 * 1024 {
            continue;
        }
        total += size;
        if total > 32 * 1024 * 1024 {
            return Err(Error::Invalid("repository text exceeds the input bound"));
        }
        let bytes = repository.blob(&CommitId::new(fields[2])?)?;
        if bytes.contains(&0) {
            continue;
        }
        if let Ok(content) = String::from_utf8(bytes) {
            if path.eq_ignore_ascii_case("README.md") {
                readme = content.clone();
            }
            materials.push(json!({"path":path,"content":content}));
        }
    }
    Ok(json!({"readme":readme, "file_list":files, "materials":materials}))
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
        assert_eq!(
            inputs["plan"]["checks"][0]["tests"],
            json!(["tests/test_greeting.py::test_hello"])
        );
        let invalid = json!({"kind":"leaf","files":[],"checks":[{"kind":"regression","node_ids":["test.py::test_it"],"goal_ids":[9]}]});
        assert!(maker_inputs(&brief, &invalid).is_err());
    }

    #[test]
    fn repository_materials_use_committed_text_without_following_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(root.path())
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--quiet"]);
        fs::write(root.path().join("README.md"), "Committed readme").unwrap();
        fs::write(root.path().join("greeting.py"), "hello\n").unwrap();
        fs::write(root.path().join("binary"), [0, 255]).unwrap();
        std::os::unix::fs::symlink("/does/not/exist", root.path().join("link")).unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "Fixture"]);
        let sha = git(&["rev-parse", "HEAD"]);
        fs::write(root.path().join("greeting.py"), "dirty\n").unwrap();
        let repo = repository_materials(root.path(), &sha).unwrap();
        assert_eq!(repo["readme"], "Committed readme");
        assert!(
            repo["materials"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["path"] == "greeting.py" && m["content"] == "hello\n")
        );
        assert!(
            !repo["materials"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["path"] == "link" || m["path"] == "binary")
        );
        assert_eq!(repo["file_list"].as_array().unwrap().len(), 4);
    }
}

#[cfg(test)]
impl Materials {
    pub(crate) fn fixture(worktrees: std::path::PathBuf) -> Self {
        Self {
            clients: Default::default(),
            worktrees,
        }
    }
}

#[cfg(test)]
mod hostile_tests {
    #[test]
    fn hostile_repository_materials() {
        crate::git_tests::hostile(
            "production::hostile_tests::hostile_repository_materials",
            || {
                let f = crate::git_tests::Repo::new();
                let value = super::repository_materials(&f.path, &f.head).unwrap();
                assert_eq!(
                    value["readme"],
                    std::fs::read_to_string(f.path.join("README.md")).unwrap()
                );
                assert_eq!(value["file_list"], serde_json::json!(["README.md"]));
            },
        );
    }
}
