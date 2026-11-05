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
            Observation::CheckPlan { node, sha, run } => {
                request(snapshot, node)?;
                if let Some(n) = world.nodes.get(node.0 as usize)
                    && n.phase == tl3_kernel::Phase::Rebasing
                    && let Some(mut fixes) = self
                        .store
                        .worker_document(&node.0.to_string(), "review_fixes")?
                    && let tl3_controller_core::translate_checks::PlanUpdate::Retarget {
                        set_reviewed,
                        replace_tests,
                    } = tl3_controller_core::translate_checks::plan_update(
                        fixes["head_sha"].as_str() == commit_sha(snapshot, n.head),
                        run.reviewed_sha.is_some(),
                        run.reproducing_unavailable.is_some(),
                    )
                {
                    fixes["head_sha"] = json!(sha);
                    if set_reviewed && let Some(reviewed) = &run.reviewed_sha {
                        fixes["reviewed_sha"] = json!(reviewed);
                    }
                    if replace_tests && let Some(reason) = &run.reproducing_unavailable {
                        for finding in fixes["findings"].as_array_mut().unwrap() {
                            if finding.get("tests").is_some() {
                                finding.as_object_mut().unwrap().remove("tests");
                                finding["reason"] = json!(reason);
                            }
                        }
                    }
                    commit
                        .rows
                        .push(put("review_fixes", node.0.to_string(), fixes));
                }
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
