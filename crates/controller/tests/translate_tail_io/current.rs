use super::*;
pub(super) struct Shell<'a>(pub &'a mut Controller);
impl std::ops::Deref for Shell<'_> {
    type Target = Controller;
    fn deref(&self) -> &Controller { self.0 }
}
impl std::ops::DerefMut for Shell<'_> {
    fn deref_mut(&mut self) -> &mut Controller { self.0 }
}
impl Shell<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn tail(&mut self, snapshot: &Snapshot, world: &World, event: Event,
        commit: &mut Commit, detailed_review: bool, failed_tests: Vec<findings::FailedTest>,
        outside_paths: Vec<String>, outside_paths_total: usize, ci_checks: Vec<findings::CiFailure>
    ) -> Result<Option<Event>> {
        let finding = {
            use tl3_controller_core::translate_tail as tail;
            let (node, input) = match event {
                Event::Reply {
                    node,
                    generation,
                    reply,
                } => (
                    Some(node),
                    tail::Input::Reply {
                        generation,
                        reply: match reply {
                            Reply::ChecksDone {
                                report,
                                pass,
                                in_footprint,
                            } => tail::Reply::ChecksDone {
                                report,
                                pass,
                                in_footprint,
                            },
                            Reply::ChangesRequested => tail::Reply::ChangesRequested,
                            Reply::MakerCantMeet => tail::Reply::MakerCantMeet,
                            _ => tail::Reply::Other,
                        },
                    },
                ),
                Event::Fact {
                    node,
                    fact: Fact::CiFailed { infra },
                } => (Some(node), tail::Input::CiFailed { infra }),
                _ => (None, tail::Input::Other),
            };
            let current = node
                .and_then(|node| world.nodes.get(node.0 as usize))
                .map(|n| tail::Node {
                    phase: match n.phase {
                        tl3_kernel::Phase::Checking => tail::Phase::Checking,
                        tl3_kernel::Phase::Reviewing => tail::Phase::Reviewing,
                        tl3_kernel::Phase::Making => tail::Phase::Making,
                        tl3_kernel::Phase::Queued => tail::Phase::Queued,
                        _ => tail::Phase::Other,
                    },
                    generation: n.generation,
                    run: n.run,
                    job: n.job,
                });
            let report = match tail::finding(current, input, detailed_review) {
                tail::Finding::Check { pass, in_footprint } => Some((findings::Kind::FailedCheck,
                    findings::check_report(pass, in_footprint, &failed_tests, &outside_paths, outside_paths_total))),
                tail::Finding::Review => Some((findings::Kind::ReviewFinding,
                    "The independent reviewer requested changes. No detailed review report was supplied.".into())),
                tail::Finding::Maker => Some((findings::Kind::ReviewFinding,
                    commit.rows.iter().find_map(|row| match row {
                        RowChange::Put { table, value, .. } if table == "maker_replies" => Some(value.to_string()),
                        _ => None,
                    }).unwrap_or_else(|| "The maker could not meet the goals.".into()))),
                tail::Finding::Ci => Some((findings::Kind::FailedCheck,
                    findings::ci_report(&ci_checks, &failed_tests))),
                tail::Finding::None => None,
            };
            node.zip(report)
                .map(|(node, (kind, report))| (node, kind, report))
        };
        if let Some((node, kind, report)) = finding {
            let materials = self.store.worker_materials(&node.0.to_string())?;
            let goals = materials
                .as_ref()
                .and_then(|materials| materials.get("maker").unwrap_or(materials).get("goals"));
            let goal_ids = match goals {
                Some(goals) => goals
                    .as_array()
                    .ok_or(Error::Invalid("maker goals must be an array"))?
                    .iter()
                    .map(|goal| {
                        goal["id"]
                            .as_str()
                            .filter(|id| !id.trim().is_empty())
                            .map(str::to_owned)
                            .ok_or(Error::Invalid("maker goal needs a string id"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                None => {
                    eprintln!(
                        "maker finding for node {} has no goal materials; retaining unbound evidence",
                        node.0
                    );
                    Vec::new()
                }
            };
            let mut findings = self
                .store
                .worker_document(&node.0.to_string(), "maker_findings")?
                .unwrap_or(json!([]));
            let finding = findings::Finding {
                id: None,
                reviewed_sha: None,
                kind,
                report: findings::clipped(&report, findings::MAX_REPORT_CHARS),
                goal_ids,
                failed_tests,
                outside_paths,
                outside_paths_total,
                ci_checks,
            };
            findings
                .as_array_mut()
                .ok_or(Error::Invalid("invalid maker findings"))?
                .push(serde_json::to_value(finding)?);
            commit
                .rows
                .push(put("maker_findings", node.0.to_string(), findings));
        }
        Ok(Some(self.priority_reserve(snapshot, event)?))
    }
}
