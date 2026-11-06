use super::*;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> {
    type Target = Controller;
    fn deref(&self) -> &Controller { self.0 }
}
impl std::ops::DerefMut for Shell<'_> {
    fn deref_mut(&mut self) -> &mut Controller { self.0 }
}
impl Shell<'_> {
    fn translate(&mut self, snapshot: &Snapshot, world: &World, observation: Observation, commit: &mut Commit, feedback: &mut Feedback) -> Result<Option<Event>> {
        let mut outside_paths_total = 0;
        let mut outside_paths = Vec::new();
        let mut failed_tests = Vec::new();
        let event = match observation {
            Observation::WorktreePrepared {
                node,
                repository,
                base_sha,
                sha,
            } => {
                let Some(n) = world.nodes.get(node.0 as usize) else {
                    return Ok(None);
                };
                use tl3_controller_core::translate_checks::{Preparation, preparation};
                if preparation(
                    n.adopted,
                    n.adopted && commit_sha(snapshot, n.head) == Some(sha.as_str()),
                ) == Preparation::Ignore
                {
                    return Ok(None);
                }
                let id = node.0.to_string();
                let mut run = if let Some(materials) = self.store.worker_materials(&id)?
                    && let Some(run) = materials.get("check_run")
                {
                    decode_stored::<adapters::prismabuild::CheckRun>(run.clone())?
                } else {
                    let brief = self
                        .store
                        .worker_document(&id, "briefs")?
                        .ok_or(Error::Invalid("adoption needs its accepted brief"))?;
                    let plan = self
                        .store
                        .worker_document(&id, "plans")?
                        .ok_or(Error::Invalid("adoption needs its accepted plan"))?;
                    let inputs = production::maker_inputs(&brief, &plan)?;
                    adapters::prismabuild::CheckRun {
                        repository: repository.clone(),
                        base_sha: tl3_worktrees::CommitId::new(&base_sha)?,
                        reviewed_sha: None,
                        reproducing_unavailable: None,
                        selected_failures: Vec::new(),
                        plan: decode_stored(inputs["plan"]["checks"].clone())?,
                    }
                };
                run.repository = repository;
                run.base_sha = tl3_worktrees::CommitId::new(base_sha)?;
                commit
                    .rows
                    .push(put("check_plans", format!("{}:{sha}", node.0), json!(run)));
                return Ok(None);
            }
            _ => unreachable!("the arm's own observation"),
        };
        *feedback = Feedback { outside_paths_total, outside_paths, failed_tests };
        Ok(Some(event))
    }
    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        tl3_worktrees::CommitId::new(sha)?;
        Ok(HeadId(
            self.store.intern_commit(&node.0.to_string(), sha)?.get() as u64,
        ))
    }
}
fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    decode_stored(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
}
fn commit_sha(snapshot: &Snapshot, head: Option<HeadId>) -> Option<&str> {
    let head = head?;
    snapshot
        .interned
        .iter()
        .find(|item| item.kind == tl3_store::InternKind::Commit && item.id == head.0 as i64)
        .map(|item| item.value.as_str())
}
fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}
pub(super) fn translate(controller: &mut Controller, snapshot: &Snapshot, world: &World, observation: Observation, commit: &mut Commit, feedback: &mut Feedback) -> Result<Option<Event>> {
    Shell(controller).translate(snapshot, world, observation, commit, feedback)
}
