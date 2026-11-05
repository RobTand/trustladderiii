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
            Observation::Checks {
                node,
                generation,
                sha,
                changed_paths,
                failed_tests: tests,
                report,
                pass,
                results,
                base_sha,
                action_key,
            } => {
                let request = request(snapshot, node)?;
                let repo = self
                    .store
                    .intern_repo(&request.repo.owner, &request.repo.name)?;
                let n = &world.nodes[node.index().ok_or(Error::Invalid("node index overflow"))?];
                let footprint: std::collections::BTreeSet<_> = world
                    .files(n.footprint)
                    .ok_or(Error::Invalid("unknown footprint"))?
                    .collect();
                let exact_head = n.head == Some(self.head(node, &sha)?);
                if n.job == Some(generation) && n.generation == generation && exact_head {
                    let prefix = format!("{}:", node.0);
                    let earlier: Vec<_> = snapshot
                        .rows
                        .iter()
                        .filter(|row| row.table == "check_reports" && row.id.starts_with(&prefix))
                        .collect();
                    for row in earlier
                        .iter()
                        .filter(|row| row.id != format!("{}:{sha}", node.0))
                    {
                        commit.rows.push(RowChange::Delete {
                            table: row.table.clone(),
                            id: row.id.clone(),
                        });
                    }
                    // Review must see an edit to a test that failed a selected check (#359).
                    let selected = adapters::prismabuild::selected_failures(
                        earlier.iter().map(|row| &row.value),
                        &results,
                    );
                    commit.rows.push(put(
                        "check_evidence",
                        format!("{}:{generation:020}", node.0),
                        evidence::check_record(
                            &json!({"head_sha":sha,"base_sha":base_sha,"action_key":action_key,
                            "report":report,"pass":pass,"results":results}),
                        ),
                    ));
                    let mut report = json!({"head_sha":sha, "base_sha": base_sha, "results": results, "action_key":action_key, "report":report, "pass":pass});
                    if !selected.is_empty() {
                        report["selected_failures"] = json!(selected);
                    }
                    commit
                        .rows
                        .push(put("check_reports", format!("{}:{sha}", node.0), report));
                }
                let mut in_footprint = true;
                for path in changed_paths {
                    let id = self.store.intern_path(repo, &path)?;
                    if !footprint.contains(&tl3_kernel::FileId(id.get() as u64)) {
                        outside_paths_total += 1;
                        if outside_paths.len() < findings::MAX_OUTSIDE_PATHS {
                            outside_paths.push(findings::clipped(&path, findings::MAX_PATH_CHARS));
                        }
                        in_footprint = false;
                    }
                }
                failed_tests = tests
                    .into_iter()
                    .take(findings::MAX_FAILED_TESTS)
                    .map(|test| findings::FailedTest {
                        test: findings::clipped(&test.test, 4096),
                        message: findings::clipped(&test.message, findings::MAX_MESSAGE_CHARS),
                    })
                    .collect();
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::ChecksDone {
                        report,
                        pass: pass && exact_head,
                        in_footprint,
                    },
                }
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
