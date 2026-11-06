//! Configured generation at exact committed controller boundaries.
use crate::{Error, Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Command};
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
}
#[derive(Serialize, Deserialize)]
struct Completed {
    input: String,
    report: Report,
}
impl Runner {
    pub fn new(config: &tl3_config::Config) -> Self {
        Self {
            config: config.clone(),
        }
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
    pub fn run(
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
        let demand = repo.test_demand.unwrap_or_default();
        let request = GenerationRequest {
            checkout,
            state: directory.join("executor.json"),
            command: repo.generator.clone().unwrap(),
            generated_files: repo.generated_files.clone(),
            files: delivery.paths.clone(),
            protected_paths: self
                .config
                .planning
                .protected_paths
                .iter()
                .chain(repo.protected_paths.iter())
                .cloned()
                .collect(),
            image: repo
                .container_image
                .clone()
                .unwrap_or_else(|| self.config.prismabuild.container_image.clone()),
            tools_path: self.config.prismabuild.tools_dir.clone(),
            cpus: demand.cpus,
            mem_gb: demand.mem_gb,
            timeout_s: repo.generator_timeout_s.unwrap_or(120),
        };
        let request_path = directory.join("request.json");
        crate::adapters::worktrees::write(&request_path, &request)?;
        let output = Command::new(&self.config.workers.python)
            .args(["-m", "tl3_workers.pb_generators"])
            .arg(&request_path)
            .output();
        let receipt = output
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| serde_json::from_slice::<GenerationReceipt>(&output.stdout).ok());
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
            if report
                .changed
                .iter()
                .any(|path| !delivery.paths.contains(path))
            {
                return Err(Error::Invalid("generator changed an undeclared output"));
            }
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

    pub(crate) fn prepare_queue_generation_repair(&mut self, delivery: &Delivery) -> Result<()> {
        if !matches!(
            delivery.output,
            crate::executor::Output::Effect {
                kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Make),
                ..
            }
        ) {
            return Ok(());
        }
        let snapshot = self.store.load()?;
        let id = delivery.node().0.to_string();
        if !snapshot
            .rows
            .iter()
            .any(|row| row.table == "generation_repairs" && row.id == id)
        {
            return Ok(());
        }
        // The queued marker and effect intent are durable before any Git write.
        let result = self
            .adapters
            .get(crate::executor::Upstream::Worktrees)?
            .prepare_generated_repair(delivery)?;
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
        Ok(())
    }
}
