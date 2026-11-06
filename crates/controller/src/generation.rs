//! Configured generation at exact committed controller boundaries.
use crate::{Error, Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
};
use tl3_contracts::{generation::GenerationRequest, roles::maker_reply::GenerationReceipt};
use tl3_worktrees::{CommitId, Worktrees};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub completed: bool,
    pub changed: Vec<String>,
    pub message: String,
    pub head: String,
    pub evidence: Option<GenerationReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_run: Option<crate::adapters::prismabuild::CheckRun>,
}
impl Report {
    pub fn fresh(&self) -> bool {
        tl3_controller_core::translate_checks::generation_fresh(
            self.completed,
            self.changed.is_empty(),
        )
    }
}

#[derive(Clone)]
pub(crate) struct Runner {
    config: tl3_config::Config,
    tasks: Arc<Mutex<HashMap<PathBuf, Task>>>,
    wakes: mpsc::Sender<()>,
    source: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
}
struct Task {
    head: String,
    result: mpsc::Receiver<Result<Option<Report>>>,
    completed: Option<Option<Report>>,
}

/// Like the worker and PB sources, only wake the owner; never mutate controller state here.
pub struct Source(mpsc::Receiver<()>);
impl crate::ingress::Source for Source {
    fn run(self, sender: crate::sync::mpsc::Sender<crate::cli::Input>) -> Result<()> {
        while self.0.recv().is_ok() {
            if sender.send(crate::cli::Input::Wake).is_err() {
                break;
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Completed {
    input: String,
    report: Report,
}
impl Runner {
    pub fn new(config: &tl3_config::Config) -> Self {
        let (wakes, source) = mpsc::channel();
        Self {
            config: config.clone(),
            tasks: Arc::default(),
            wakes,
            source: Arc::new(Mutex::new(Some(source))),
        }
    }
    pub fn source(&self) -> Option<Source> {
        self.source.lock().unwrap().take().map(Source)
    }
    pub fn run(
        &self,
        git: &Worktrees,
        directory: &Path,
        delivery: &Delivery,
        head: &CommitId,
    ) -> Result<Option<Report>> {
        if !self.configured(delivery) {
            return Ok(None);
        }
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| Error::Invalid("generation tasks poisoned"))?;
        if let Some(task) = tasks.get_mut(directory) {
            if task.head != head.as_str()
                && !task
                    .completed
                    .as_ref()
                    .and_then(|r| r.as_ref())
                    .is_some_and(|r| r.head == head.as_str())
            {
                return Err(Error::Invalid("generation task changed its input head"));
            }
            if let Some(report) = &task.completed {
                return Ok(report.clone());
            }
            match task.result.try_recv() {
                Ok(Ok(report)) => {
                    task.completed = Some(report.clone());
                    return Ok(report);
                }
                Ok(Err(error)) => {
                    tasks.remove(directory);
                    return Err(error);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    tasks.remove(directory);
                    return Err(Error::Invalid("generation source ended without a result"));
                }
                Err(mpsc::TryRecvError::Empty) => return Err(Error::GenerationPending),
            }
        }
        let runner = self.clone();
        let directory = directory.to_owned();
        let common = git.common_directory().to_owned();
        let delivery = delivery.clone();
        let head = head.clone();
        let (sender, result) = mpsc::channel();
        tasks.insert(
            directory.clone(),
            Task {
                head: head.as_str().into(),
                result,
                completed: None,
            },
        );
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Private snapshots and object import may block; branch refs are installed only by the owner.
                let git = Worktrees::open(&common, &common)?;
                runner.run_blocking(&git, &directory, &delivery, &head)
            }))
            .unwrap_or(Err(Error::Invalid("generation source panicked")));
            let _ = sender.send(result);
            let _ = runner.wakes.send(());
        });
        Err(Error::GenerationPending)
    }
    fn repository(&self, delivery: &Delivery) -> Option<&tl3_config::Repository> {
        self.config.github.repos.iter().find(|repo| {
            repo.owner.eq_ignore_ascii_case(&delivery.repository.owner)
                && repo.name.eq_ignore_ascii_case(&delivery.repository.name)
                && repo.installation_id == delivery.repository.installation
        })
    }
    pub fn configured_repository(&self, identity: &crate::Repository) -> bool {
        self.config.github.repos.iter().any(|repo| {
            repo.generator.is_some()
                && repo.owner.eq_ignore_ascii_case(&identity.owner)
                && repo.name.eq_ignore_ascii_case(&identity.name)
                && repo.installation_id == identity.installation
        })
    }
    pub fn configured(&self, delivery: &Delivery) -> bool {
        self.repository(delivery)
            .is_some_and(|repo| repo.generator.is_some())
    }
    fn run_blocking(
        &self,
        git: &Worktrees,
        directory: &Path,
        delivery: &Delivery,
        head: &CommitId,
    ) -> Result<Option<Report>> {
        let Some(repo) = self
            .repository(delivery)
            .filter(|repo| repo.generator.is_some())
        else {
            return Ok(None);
        };
        let demand = repo.test_demand.unwrap_or_default();
        let mut request = GenerationRequest {
            checkout: Default::default(),
            state: Default::default(),
            command: repo.generator.clone().unwrap(),
            generated_files: repo.generated_files.clone(),
            files: delivery.paths.clone(),
            protected_paths: tl3_config::protected_paths(
                &self.config.planning.protected_paths,
                &repo.protected_paths,
            ),
            image: repo
                .container_image
                .clone()
                .unwrap_or_else(|| self.config.prismabuild.container_image.clone()),
            tools_path: self.config.prismabuild.tools_dir.clone(),
            cpus: demand.cpus,
            mem_gb: demand.mem_gb,
            timeout_s: repo.generator_timeout_s.unwrap_or(120),
        };
        let contract = format!("{:x}", Sha256::digest(serde_json::to_vec(&request)?));
        let directory = directory.join(contract);
        let directory = directory.as_path();
        request.checkout = directory.join("checkout");
        request.state = directory.join("executor.json");
        fs::create_dir_all(directory)?;
        let completed = directory.join("completed.json");
        if let Some(saved) = crate::adapters::worktrees::read::<Completed>(&completed)? {
            if head.as_str() != saved.input && head.as_str() != saved.report.head {
                return Err(Error::Invalid(
                    "generation operation changed its input head",
                ));
            }
            if saved.report.completed {
                let snapshot =
                    Worktrees::open(directory.join("checkout"), directory.join("checkout"))?;
                git.import_commit(
                    snapshot.common_directory(),
                    &CommitId::new(saved.report.head.clone())?,
                )?;
            }
            return Ok(Some(saved.report));
        }
        let intent = directory.join("head.json");
        if let Some(saved) = crate::adapters::worktrees::read::<String>(&intent)? {
            if saved != head.as_str() {
                return Err(Error::Invalid("generation input changed during recovery"));
            }
        } else {
            crate::adapters::worktrees::write(&intent, &head.as_str())?;
        }
        let checkout = directory.join("checkout");
        if !checkout.exists() {
            let stage = directory.join("preparing");
            if stage.exists() {
                fs::remove_dir_all(&stage)?;
            }
            git.clone_checkout(&stage, head)?;
            fs::rename(&stage, &checkout)?;
            fs::File::open(directory)?.sync_all()?;
        }
        let snapshot = Worktrees::open(&checkout, &checkout)?;
        if snapshot.head()? != *head {
            return Err(Error::Invalid("generation snapshot head changed"));
        }
        let request_path = directory.join("request.json");
        crate::adapters::worktrees::write(&request_path, &request)?;
        // Reuse worker process identities, without OS file locks on the deployment kernel.
        // A surviving old executor owns only this private snapshot. Wait off the drive loop.
        let claim = directory.join("executor-process.json");
        while crate::adapters::workers::process::alive(&claim)? {
            thread::sleep(std::time::Duration::from_millis(100));
        }
        match fs::remove_file(&claim) {
            Ok(()) => fs::File::open(directory)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let output = Command::new(&self.config.workers.python)
            .args(["-m", "tl3_workers.pb_generators"])
            .arg(&request_path)
            .output();
        let receipt = output
            .as_ref()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| serde_json::from_slice::<GenerationReceipt>(&output.stdout).ok());
        if receipt.is_none() {
            // Retain bounded executor evidence, including failure replies which are not success receipts.
            let (reply, diagnostics) = match &output {
                Ok(output) => (
                    serde_json::from_slice::<serde_json::Value>(&output.stdout).ok(),
                    format!(
                        "stdout: {}\nstderr: {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                ),
                Err(error) => (None, error.to_string()),
            };
            crate::adapters::worktrees::write(
                &directory.join("failed.json"),
                &serde_json::json!({
                    "action_key": reply.as_ref().and_then(|r| r.get("action_key")).and_then(|v| v.as_str()).map(|v| crate::findings::clipped(v, 128)),
                    "message": crate::findings::clipped(reply.as_ref().and_then(|r| r.get("message")).and_then(|v| v.as_str()).unwrap_or("Generator executor did not return a success receipt"), 2000),
                    "diagnostics": crate::findings::clipped(&diagnostics, 16000),
                }),
            )?;
        }
        let mut report = Report {
            completed: false,
            changed: Vec::new(),
            message: format!(
                "Configured generator failed for outputs {}. Repair its inputs and run run_generator; inspect the generation action evidence. Do not hand-edit generated outputs.",
                serde_json::to_string(&repo.generated_files)?
            ),
            head: head.as_str().into(),
            evidence: receipt,
            check_run: None,
        };
        if let Some(receipt) = &report.evidence {
            if git.tree(head)? != receipt.input_tree.as_str() {
                return Err(Error::Invalid(
                    "generator receipt has a different input tree",
                ));
            }
            let generated = snapshot.commit_generation_input(head)?;
            if snapshot.tree(&generated)? != receipt.output_tree.as_str() {
                return Err(Error::Invalid(
                    "generated tree differs from executor receipt",
                ));
            }
            report.changed = snapshot
                .changed_files(head, &generated)?
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            // The existing executor validates configured globs, declared scope and protection.
            // Independently restrict the commit to declared output paths at the Rust boundary.
            let undeclared: Vec<_> = report
                .changed
                .iter()
                .filter(|path| !delivery.paths.contains(path))
                .cloned()
                .collect();
            if !undeclared.is_empty() {
                report.message = format!(
                    "Generator changed undeclared outputs: {}. Request scope growth for these paths, then run run_generator. Do not hand-edit generated outputs.",
                    undeclared.join(", ")
                );
            } else {
                git.import_commit(snapshot.common_directory(), &generated)?;
                report.completed = true;
                report.head = generated.as_str().into();
                report.message = if report.changed.is_empty() {
                    "Configured generated outputs are fresh.".into()
                } else {
                    format!(
                        "Stale generated outputs: {}. Repair generator inputs and call run_generator before completing the maker repair. Do not hand-edit generated outputs.",
                        report.changed.join(", ")
                    )
                };
            }
        }
        crate::adapters::worktrees::write(
            &completed,
            &Completed {
                input: head.as_str().into(),
                report: report.clone(),
            },
        )?;
        Ok(Some(report))
    }
}

impl crate::Controller {
    pub(crate) fn generation_delivery(
        &self,
        snapshot: &tl3_store::Snapshot,
        world: &tl3_kernel::World,
        node: tl3_kernel::NodeId,
        generation: u64,
        sha: &str,
    ) -> Result<Delivery> {
        let request = crate::request(snapshot, node)?;
        let n = &world.nodes[node.0 as usize];
        let paths = crate::declared_scope::paths(snapshot, None, world, node, &snapshot.interned)?;
        Ok(Delivery {
            key: format!("freshness:{}:{generation}", node.0),
            epoch: snapshot.epoch.get(),
            output: crate::executor::Output::Effect {
                node,
                seq: 0,
                kind: tl3_kernel::EffectKind::SubmitChecks,
                head: n.head,
            },
            generation,
            head_sha: Some(sha.into()),
            repository: request.repo,
            issue: request.issue,
            pull_request: None,
            proposed: request.proposed,
            paths,
            attempt: n.attempt,
            round: n.round,
            replanned: n.replanned,
            checks: None,
            job_generation: n.job,
            question: None,
            note: None,
            scope_removals: None,
            marker_key: None,
            question_context: None,
            steering: None,
            request_text: None,
            evidence: None,
        })
    }

    pub(crate) fn prepare_queue_generation_repair(&mut self, delivery: &Delivery) -> Result<bool> {
        if !matches!(
            delivery.output,
            crate::executor::Output::Effect {
                kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Make),
                ..
            }
        ) {
            return Ok(true);
        }
        let snapshot = self.store.load()?;
        let id = delivery.node().0.to_string();
        if !snapshot
            .rows
            .iter()
            .any(|row| row.table == "generation_repairs" && row.id == id)
        {
            return Ok(true);
        }
        // The queued marker and effect intent are durable before any Git write.
        let result = match self
            .adapters
            .get(crate::executor::Upstream::Worktrees)?
            .prepare_generated_repair(delivery)
        {
            Err(Error::GenerationPending) => return Ok(false),
            result => result?,
        };
        let mut commit = tl3_store::Commit::new(
            format!("generation-repair:{}", delivery.key),
            serde_json::json!({"generation_repair":delivery.key}),
            &snapshot,
        );
        commit.rows.push(tl3_store::RowChange::Delete {
            table: "generation_repairs".into(),
            id,
        });
        if let Some(report) = result {
            if let (Some(run), Some(head)) = (&report.check_run, &delivery.head_sha) {
                commit.rows.push(crate::put(
                    "check_plans",
                    format!("{}:{head}", delivery.node().0),
                    serde_json::json!(run),
                ));
            }
            commit.rows.push(crate::put(
                "generation_repair_evidence",
                delivery.key.clone(),
                serde_json::json!(report),
            ));
        }
        self.store.commit(&commit)?;
        Ok(true)
    }
}

impl crate::Controller {
    pub(crate) fn park_generation_check(&mut self, row: &tl3_store::WorldRow) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = tl3_store::Commit::new(
            format!("generation-wait:{}:{}", row.id, snapshot.revision.get()),
            serde_json::json!({"generation_wait":row.id}),
            &snapshot,
        );
        commit.rows.push(crate::put(
            "generation_checks",
            row.id.clone(),
            row.value.clone(),
        ));
        commit.rows.push(tl3_store::RowChange::Delete {
            table: row.table.clone(),
            id: row.id.clone(),
        });
        self.store.commit(&commit)?;
        Ok(())
    }
    pub(crate) fn poll_generation_checks(
        &mut self,
        snapshot: &tl3_store::Snapshot,
    ) -> Result<bool> {
        let world = crate::snapshot::decode(snapshot)?;
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "generation_checks")
        {
            let observation: crate::executor::Observation =
                crate::decode_stored(row.value.clone())?;
            let crate::executor::Observation::Checks {
                node,
                generation,
                ref sha,
                ..
            } = observation
            else {
                return Err(Error::Invalid("invalid pending generation check"));
            };
            let n = world
                .nodes
                .get(node.0 as usize)
                .ok_or(Error::Invalid("generation check node absent"))?;
            let current = tl3_controller_core::translate_checks::checks_reply(
                n.job,
                n.generation,
                generation,
                crate::commit_sha(snapshot, n.head) == Some(sha.as_str()),
                true,
            )
            .record;
            if current {
                let delivery = self.generation_delivery(snapshot, &world, node, generation, sha)?;
                match self
                    .adapters
                    .get(crate::executor::Upstream::Worktrees)?
                    .check_generation(&delivery)
                {
                    Err(Error::GenerationPending) => continue,
                    result => {
                        result?;
                    }
                }
            }
            let mut commit = tl3_store::Commit::new(
                format!("generation-ready:{}:{}", row.id, snapshot.revision.get()),
                serde_json::json!({"generation_ready":row.id}),
                snapshot,
            );
            commit
                .rows
                .push(crate::put("inbox", row.id.clone(), row.value.clone()));
            commit.rows.push(tl3_store::RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }
}
