use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueFact {
    pub repository: Repository,
    pub number: u64,
    pub evidence: Evidence,
}
#[derive(Serialize)]
pub enum Input {
    Look {
        key: String,
    },
    Observation {
        key: String,
        observation: Observation,
    },
}
#[derive(Serialize)]
pub enum Observation {
    Queue(QueueFact),
}
impl QueueFact {
    pub(crate) fn node(&self, snapshot: &Snapshot) -> Result<Option<NodeId>> {
        for row in snapshot.rows.iter().filter(|row| {
            row.table == "pull_requests" && row.value["number"].as_u64() == Some(self.number)
        }) {
            let node = NodeId(
                row.id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid PR binding"))?,
            );
            if super::request(snapshot, node)?.repo == self.repository {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
    pub(crate) fn recorded(&self, snapshot: &Snapshot) -> Result<bool> {
        if let Evidence::CheckMissing { head, required }
        | Evidence::CheckReported { head, required } = &self.evidence
        {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            let id = super::looks::missing_check_incident_id(node, head, required);
            let incident = snapshot
                .rows
                .iter()
                .find(|row| row.table == "incidents" && row.id == id);
            // A raised incident stays deduplicated after a clear; a report matters only while open.
            return Ok(match self.evidence {
                Evidence::CheckMissing { .. } => incident.is_some(),
                _ => !incident.is_some_and(|row| row.value["open"] == true),
            });
        }
        if let Evidence::QueueBlocked {
            head, conditions, ..
        } = &self.evidence
        {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            let id = super::looks::queue_blocked_incident_id(node, head, conditions);
            return Ok(snapshot
                .rows
                .iter()
                .any(|row| row.table == "incidents" && row.id == id));
        }
        if let Evidence::ChecksUnavailable { permissions } = &self.evidence {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            return Ok(permissions.iter().all(|permission| {
                let id = super::looks::check_setup_incident_id(node, permission);
                snapshot.rows.iter().any(|row| {
                    row.table == "incidents" && row.id == id && row.value["open"] == true
                })
            }));
        }
        let identity = self.identity();
        Ok(snapshot
            .rows
            .iter()
            .any(|row| row.table == "queue_facts" && row.id == identity))
    }
    pub(crate) fn identity(&self) -> String {
        let mut evidence = match &self.evidence {
            Evidence::ChecksUnavailable { permissions } => {
                let mut gaps = permissions.clone();
                gaps.sort();
                gaps.dedup();
                format!(
                    "required-checks-setup:{}",
                    serde_json::to_string(&gaps).unwrap()
                )
            }
            Evidence::CheckMissing { head, required } => {
                format!(
                    "missing-check:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::CheckReported { head, required } => {
                format!(
                    "check-reported:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::QueueBlocked {
                head, conditions, ..
            } => format!(
                "queue-blocked:{head}:{}",
                serde_json::to_string(conditions).unwrap()
            ),
            Evidence::Merged { head, .. } => format!("merged:{head}"),
            Evidence::CiFailed {
                check, head_check, ..
            } => {
                format!("{}:{check}", if *head_check { "head-ci" } else { "ci" })
            }
            Evidence::Conflict { check, .. } => format!("conflict:{check}"),
            Evidence::DequeuedByCommand { check, .. } => format!("command-dequeued:{check}"),
            Evidence::DequeuedByPerson { event } => format!("dequeued:{event}"),
            Evidence::ClosedByPerson { event } => format!("closed:{event}"),
            Evidence::PushByOther { head } => format!("push:{head}"),
        };
        // Mergify can reuse one check ID across successive queue attempts.
        if let Evidence::CiFailed {
            completed_at: Some(time),
            ..
        }
        | Evidence::Conflict {
            completed_at: Some(time),
            ..
        }
        | Evidence::DequeuedByCommand {
            completed_at: Some(time),
            ..
        } = &self.evidence
        {
            evidence.push(':');
            evidence.push_str(time);
        }
        format!(
            "{}/{}/{}/{}/{evidence}",
            self.repository.owner.to_ascii_lowercase(),
            self.repository.name.to_ascii_lowercase(),
            self.repository.installation,
            self.number
        )
    }
    fn input(self) -> Input {
        Input::Observation {
            key: format!("mergify:{}", self.identity()),
            observation: Observation::Queue(self),
        }
    }
}
struct Tracked {
    number: u64,
    head: String,
    approved: String,
    known_heads: Vec<String>,
    missing: Vec<RequiredCheck>,
}
pub struct Translator {
    clients: BTreeMap<u64, GitHub>,
    runtime: &'static tokio::runtime::Runtime,
}
impl Translator {
    pub fn webhook(
        &self,
        event: &Event,
        snapshot: &Snapshot,
        repos: &[Repository],
    ) -> Result<Vec<Input>> {
        if !route::webhook::interested(route_event(event)) {
            return Ok(vec![]);
        }
        let context = match event {
            Event::PullRequestMerged { context, .. }
            | Event::PullRequestClosed { context, .. }
            | Event::PullRequestSynchronized { context, .. }
            | Event::PullRequestUnlabeled { context, .. }
            | Event::CheckRunCompleted { context, .. }
            | Event::CheckRunUpdated { context, .. }
            | Event::CheckSuiteCompleted { context, .. }
            | Event::MergeGroup { context, .. } => context,
            _ => return Ok(vec![]),
        };
        let Some(repo) = repos.iter().find(|repo| {
            route::webhook::repository(
                repo.installation == context.installation_id,
                repo.owner
                    .eq_ignore_ascii_case(&context.repository.owner.login),
                repo.name.eq_ignore_ascii_case(&context.repository.name),
            )
        }) else {
            return Ok(vec![]);
        };
        let github = self
            .clients
            .get(&repo.installation)
            .ok_or(Error::Invalid("missing queue fact client"))?;
        let open = tracked(snapshot, repo, false)?;
        let world: World = super::snapshot::decode(snapshot)?;
        let mut facts = Vec::new();
        let mut wake = Vec::new();
        match event {
            Event::PullRequestMerged { pull_request, .. } => {
                let binding = QueueFact {
                    repository: repo.clone(),
                    number: pull_request.number,
                    evidence: Evidence::Merged {
                        head: pull_request.head.sha.clone(),
                        checks_passed: false,
                    },
                };
                if let Some(node) = binding.node(snapshot)?
                    && route::webhook::merged(
                        true,
                        world.nodes[node.0 as usize].merged.is_none(),
                        is_merge_queue_pr(pull_request),
                    )
                {
                    let checks = self
                        .runtime
                        .block_on(github.check_runs(&full_name(repo), &pull_request.head.sha))?;
                    let head = approved_head(snapshot, world.nodes[node.0 as usize].approved)?;
                    let state = tl3_mergify::observe(pull_request.clone(), &checks, &head);
                    facts.extend(self.runtime.block_on(queue_facts(
                        github,
                        repo,
                        &state,
                        &head,
                        &[],
                        Some(snapshot),
                    ))?);
                }
            }
            Event::PullRequestClosed {
                pull_request,
                actor,
                ..
            }
            | Event::PullRequestUnlabeled {
                pull_request,
                actor,
                ..
            }
            | Event::PullRequestSynchronized {
                pull_request,
                actor,
                ..
            } => {
                if !route::webhook::pull(is_merge_queue_pr(pull_request), true) {
                    return Ok(vec![]);
                }
                let Some(item) = open
                    .iter()
                    .find(|item| route::projection::number(item.number, pull_request.number))
                else {
                    return Ok(vec![]);
                };
                let evidence = match event {
                    Event::PullRequestClosed { .. } | Event::PullRequestUnlabeled { .. } => {
                        let events = self
                            .runtime
                            .block_on(github.issue_events(&full_name(repo), pull_request.number))?;
                        // A delayed webhook can predate the close in these fresh events.
                        let current = self
                            .runtime
                            .block_on(github.pull_request(&full_name(repo), pull_request.number))?;
                        if route::webhook::fresh_merge(current.merged_at.is_some()) {
                            facts.extend(self.look(github, repo, item, snapshot)?);
                        } else {
                            facts.extend(person_facts(repo, pull_request.number, &events, false));
                        }
                        None
                    }
                    Event::PullRequestSynchronized { after, .. }
                        if route::webhook::push(
                            actor.login == format!("{}[bot]", github.app_slug()),
                            after == &pull_request.head.sha,
                        ) =>
                    {
                        Some(Evidence::PushByOther {
                            head: after.clone(),
                        })
                    }
                    _ => None,
                };
                facts.extend(evidence.map(|evidence| QueueFact {
                    repository: repo.clone(),
                    number: pull_request.number,
                    evidence,
                }));
            }
            Event::CheckRunCompleted { check_run, .. }
            | Event::CheckRunUpdated { check_run, .. } => {
                let check_route = route_check(check_run);
                if check_route == route::webhook::Check::Drop {
                    return Ok(vec![]);
                }
                let queue_event = check_route == route::webhook::Check::Queue;
                for item in tracked(snapshot, repo, true)?.iter().filter(|item| {
                    route::webhook::head_pull(
                        item.head == check_run.head_sha,
                        check_run.pull_requests.is_empty(),
                        check_run
                            .pull_requests
                            .iter()
                            .any(|pull| pull.number == item.number),
                    )
                }) {
                    let pending = pending_enqueues(snapshot, repo, item.number)?;
                    let action = route::webhook::check_action(
                        queue_event,
                        !pending.is_empty(),
                        item.approved == check_run.head_sha,
                    );
                    if action.look {
                        facts.extend(self.look(github, repo, item, snapshot)?);
                        if action.wake {
                            wake.extend(pending);
                        }
                    }
                }
            }
            // A completed suite can be the last evidence that a required check won't report (Q52).
            Event::CheckSuiteCompleted { check_suite, .. } => {
                for item in open.iter().filter(|item| {
                    route::webhook::head_pull(
                        item.approved == check_suite.head_sha,
                        check_suite.pull_requests.is_empty(),
                        check_suite
                            .pull_requests
                            .iter()
                            .any(|pull| pull.number == item.number),
                    )
                }) {
                    let pending = pending_enqueues(snapshot, repo, item.number)?;
                    if route::webhook::suite_action(!pending.is_empty()).look {
                        facts.extend(self.look(github, repo, item, snapshot)?);
                        wake.extend(pending);
                    }
                }
            }
            Event::MergeGroup { .. } => {
                for item in &open {
                    facts.extend(self.look(github, repo, item, snapshot)?);
                }
            }
            _ => {}
        }
        wake.extend(facts.into_iter().map(QueueFact::input));
        Ok(wake)
    }
    fn look(
        &self,
        github: &GitHub,
        repo: &Repository,
        item: &Tracked,
        snapshot: &Snapshot,
    ) -> Result<Vec<QueueFact>> {
        let state = self.runtime.block_on(read_queue(
            github,
            &full_name(repo),
            item.number,
            &item.head,
        ))?;
        self.runtime.block_on(queue_facts(
            github,
            repo,
            &state,
            &item.approved,
            &item.missing,
            Some(snapshot),
        ))
    }
    pub fn catch_up(
        &self,
        repo: &Repository,
        changes: &RepoChanges,
        snapshot: &Snapshot,
    ) -> Result<Vec<Input>> {
        let github = self
            .clients
            .get(&repo.installation)
            .ok_or(Error::Invalid("missing queue fact client"))?;
        let mut facts = Vec::new();
        let world: World = super::snapshot::decode(snapshot)?;
        for pull in changes.pull_requests.iter().filter(|pull| {
            route::projection::catch_merged(pull.merged_at.is_some(), is_merge_queue_pr(pull))
        }) {
            let binding = QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence: Evidence::Merged {
                    head: pull.head.sha.clone(),
                    checks_passed: false,
                },
            };
            if let Some(node) = binding.node(snapshot)?
                && route::projection::merged(
                    true,
                    matches!(
                        world.nodes[node.0 as usize].phase,
                        tl3_kernel::Phase::Closed(_)
                    ),
                    world.nodes[node.0 as usize].merged.is_none(),
                )
            {
                let head = approved_head(snapshot, world.nodes[node.0 as usize].approved)?;
                let state = tl3_mergify::observe(pull.clone(), &changes.check_runs, &head);
                facts.extend(self.runtime.block_on(queue_facts(
                    github,
                    repo,
                    &state,
                    &head,
                    &[],
                    Some(snapshot),
                ))?);
            }
        }
        for item in tracked(snapshot, repo, false)? {
            // Reread tracked PRs even when their updated_at is outside the catch-up window.
            facts.extend(self.look(github, repo, &item, snapshot)?);
            let pull = self
                .runtime
                .block_on(github.pull_request(&full_name(repo), item.number))?;
            if !route::projection::catch_pull(is_merge_queue_pr(&pull), pull.merged_at.is_some()) {
                continue;
            }
            if route::projection::unknown(item.known_heads.contains(&pull.head.sha)) {
                facts.push(QueueFact {
                    repository: repo.clone(),
                    number: item.number,
                    evidence: Evidence::PushByOther {
                        head: pull.head.sha.clone(),
                    },
                });
            }
            for (number, event) in &changes.labels {
                if route::projection::number(*number, item.number)
                    && let Some(facts::PersonFact::Dequeued { event }) =
                        facts::person_fact(facts::IssueEvent {
                            id: event.id,
                            actor: actor(Some(&event.actor)),
                            kind: event_kind(&event.event, Some(event.label.name.as_str())),
                        })
                {
                    facts.push(QueueFact {
                        repository: repo.clone(),
                        number: item.number,
                        evidence: Evidence::DequeuedByPerson { event },
                    });
                }
            }
            for (number, event) in &changes.closed {
                if route::projection::person(
                    *number == item.number,
                    actor(event.actor.as_ref()) == facts::Actor::Person,
                ) {
                    facts.push(QueueFact {
                        repository: repo.clone(),
                        number: item.number,
                        evidence: Evidence::ClosedByPerson { event: event.id },
                    });
                }
            }
        }
        Ok(facts.into_iter().map(QueueFact::input).collect())
    }
}
async fn read_queue(
    github: &GitHub,
    repo: &str,
    number: u64,
    head: &str,
) -> Result<tl3_mergify::Observation> {
    let mergify = Mergify::new(github.clone());
    let mut retries = 2;
    loop {
        match mergify.read(repo, number, head).await {
            Err(tl3_mergify::Error::ChangedDuringRead) if retries > 0 => retries -= 1,
            result => return result.map_err(adapter_error),
        }
    }
}
pub(crate) fn checks_settled(state: &tl3_mergify::Observation) -> bool {
    facts::checks_settled(&tl3_mergify::views(
        &state.check_runs,
        &state.pull_request.head.sha,
        |_| false,
    ))
}
pub(crate) fn queue_blocked(
    state: &tl3_mergify::Observation,
    suites_completed: bool,
    required: &[RequiredCheck],
) -> Option<(Vec<String>, Vec<String>)> {
    let conditions =
        if route::queue::conditions(state.state == QueueState::Waiting, suites_completed) {
            tl3_mergify::unmet_conditions(state.check.as_ref()?)
        } else {
            vec![]
        };
    if !facts::queue_blocked(state.state, suites_completed, !conditions.is_empty()) {
        return None;
    }
    let views = tl3_mergify::views(&state.check_runs, &state.pull_request.head.sha, |check| {
        required.iter().any(|rule| rule.matches(check))
    });
    // Each failing name is the newest of its name, so sorting gives the
    // former BTreeMap's order by name.
    let mut failed: Vec<String> = (0..views.len())
        .filter(|&index| facts::blocking_failure(&views, index))
        .map(|index| state.check_runs[index].name.clone())
        .collect();
    failed.sort();
    Some((conditions, failed))
}
async fn queue_facts(
    github: &GitHub,
    repo: &Repository,
    state: &tl3_mergify::Observation,
    expected_head: &str,
    open: &[RequiredCheck],
    snapshot: Option<&Snapshot>,
) -> Result<Vec<QueueFact>> {
    let pull = &state.pull_request;
    let head_checks = match facts::entry(
        is_merge_queue_pr(pull),
        state.state,
        pull.head.sha == expected_head,
        pull.state == "open",
    ) {
        facts::Entry::Ignore => return Ok(vec![]),
        facts::Entry::PushByOther => {
            return Ok(vec![QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence: Evidence::PushByOther {
                    head: pull.head.sha.clone(),
                },
            }]);
        }
        facts::Entry::Read { head_checks } => head_checks,
    };
    let mut facts = Vec::new();
    let mut head_infrastructure = None;
    if head_checks {
        let repo_name = full_name(repo);
        let requirements = github
            .required_status_checks(&repo_name, &pull.base.branch)
            .await?;
        let checks =
            head_checks::evaluate(github, &repo_name, pull, requirements, &state.check_runs)
                .await?;
        head_infrastructure = checks.infrastructure();
        let setup = route::queue::setup(checks.unavailable.is_empty(), checks.missing.is_empty());
        if setup.unavailable {
            facts.push(QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence: Evidence::ChecksUnavailable {
                    permissions: checks.unavailable.clone(),
                },
            });
        }
        // Q52: once every suite on the head has completed, no app is still
        // working on it, so a missing required context will not report.
        if setup.missing_suites {
            let suites = github.check_suites(&repo_name, &pull.head.sha).await?;
            if facts::suites_completed(&suite_statuses(&suites)) {
                facts.extend(checks.missing.iter().map(|required| QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence: Evidence::CheckMissing {
                        head: pull.head.sha.clone(),
                        required: required.clone(),
                    },
                }));
            }
        }
        // #447: Mergify waits on a condition nothing still running can meet,
        // such as PrismaBuild's scopes after its pr-gate failed.
        if route::queue::blocked_suites(
            state.state == QueueState::Waiting,
            checks_settled(state),
            state
                .check
                .as_ref()
                .is_some_and(|check| !tl3_mergify::unmet_conditions(check).is_empty()),
        ) {
            let suites = github.check_suites(&repo_name, &pull.head.sha).await?;
            let completed = facts::suites_completed(&suite_statuses(&suites));
            let required = route::queue::required(checks.missing.clone(), checks.reported.clone());
            if let Some((conditions, failed)) = queue_blocked(state, completed, &required) {
                facts.push(QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence: Evidence::QueueBlocked {
                        head: pull.head.sha.clone(),
                        conditions,
                        failed,
                    },
                });
            }
        }
        facts.extend(
            open.iter()
                .filter(|required| route::queue::reported(true, checks.reported.contains(required)))
                .map(|required| QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence: Evidence::CheckReported {
                        head: pull.head.sha.clone(),
                        required: required.clone(),
                    },
                }),
        );
        if let Some(evidence) = checks.evidence(pull) {
            let mut fact = QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence,
            };
            if route::queue::emit(
                snapshot
                    .map(|snapshot| fact.recorded(snapshot))
                    .transpose()?
                    .unwrap_or(false),
            ) {
                if let Evidence::CiFailed { details, .. } = &mut fact.evidence {
                    *details =
                        ci_evidence::read_checks(github, &repo_name, &state.check_runs).await;
                }
                facts.push(fact);
            }
            return Ok(facts);
        }
    }
    let outcome = facts::queue_outcome(
        state.state,
        state.check.as_ref(),
        head_infrastructure,
        tl3_mergify::ci_is_infrastructure,
    );
    let attempt = |check: &CheckRun| (check.id, check.completed_at.map(|time| time.to_rfc3339()));
    let evidence = match outcome {
        Some(facts::Outcome::Merged) => {
            let queue_check = state.check.as_ref().map(|check| facts::QueueCheck {
                completed: check.status == "completed",
                concluded: check.conclusion.is_some(),
                merged_title: check.output.as_ref().is_some_and(|output| {
                    route::queue::merged_title(
                        output.title.as_deref() == Some("Merged via merge queue"),
                    )
                }),
            });
            let checks_passed = match facts::merged_plan(
                pull.head.sha == expected_head,
                queue_check,
                pull.merge_commit_sha.is_some(),
            ) {
                // Mergify publishes its final queue result after GitHub records the merge.
                facts::MergedPlan::Wait => return Ok(vec![]),
                facts::MergedPlan::ViaQueue => tl3_mergify::merged_via_queue(state),
                facts::MergedPlan::MergeCommit => {
                    let sha = pull.merge_commit_sha.as_deref().unwrap_or_default();
                    checks_passed(&github.check_runs(&full_name(repo), sha).await?, sha)
                }
                facts::MergedPlan::NotPassed => false,
            };
            Some(Evidence::Merged {
                head: pull.head.sha.clone(),
                checks_passed,
            })
        }
        Some(facts::Outcome::CiFailed { check, infra }) => {
            let (check, completed_at) = attempt(check);
            Some(Evidence::CiFailed {
                check,
                completed_at,
                head: pull.head.sha.clone(),
                head_check: false,
                details: Default::default(),
                infra,
            })
        }
        Some(facts::Outcome::DequeuedByCommand(check)) => {
            let (check, completed_at) = attempt(check);
            Some(Evidence::DequeuedByCommand {
                check,
                completed_at,
                head: pull.head.sha.clone(),
            })
        }
        Some(facts::Outcome::Conflict(check)) => {
            let (check, completed_at) = attempt(check);
            Some(Evidence::Conflict {
                check,
                completed_at,
                head: pull.head.sha.clone(),
            })
        }
        None => None,
    };
    if let Some(evidence) = evidence {
        let mut fact = QueueFact {
            repository: repo.clone(),
            number: pull.number,
            evidence,
        };
        if route::queue::emit(
            snapshot
                .map(|snapshot| fact.recorded(snapshot))
                .transpose()?
                .unwrap_or(false),
        ) {
            if let Evidence::CiFailed { details, .. } = &mut fact.evidence {
                *details = ci_evidence::read_queue(github, &full_name(repo), state).await;
            }
            facts.push(fact);
        }
    }
    Ok(facts)
}
fn checks_passed(checks: &[CheckRun], sha: &str) -> bool {
    facts::checks_passed(&tl3_mergify::views(checks, sha, |_| false))
}
fn suite_statuses(suites: &[tl3_github::CheckSuite]) -> Vec<tl3_mergify::decide::Status> {
    suites
        .iter()
        .map(|suite| tl3_mergify::decide::Status::parse(&suite.status))
        .collect()
}
fn approved_head(snapshot: &Snapshot, approved: Option<tl3_kernel::HeadId>) -> Result<String> {
    let Some(head) = approved else {
        return Ok(String::new());
    };
    snapshot
        .interned
        .iter()
        .find(|entry| entry.kind == InternKind::Commit && entry.id == head.0 as i64)
        .map(|entry| entry.value.clone())
        .ok_or(Error::Invalid("approved head is not interned"))
}
fn tracked(snapshot: &Snapshot, repo: &Repository, include_closed: bool) -> Result<Vec<Tracked>> {
    let world: World = super::snapshot::decode(snapshot)?;
    let mut result = Vec::new();
    for row in snapshot
        .rows
        .iter()
        .filter(|row| row.table == "pull_requests")
    {
        let node = NodeId(
            row.id
                .parse()
                .map_err(|_| Error::Invalid("invalid PR node"))?,
        );
        let n = &world.nodes[node.0 as usize];
        if !route::projection::phase(
            matches!(n.phase, tl3_kernel::Phase::Closed(_)),
            include_closed,
            n.merged.is_some(),
        ) || super::request(snapshot, node)?.repo != *repo
        {
            continue;
        }
        let Some(head) = world.nodes[node.0 as usize].head else {
            continue;
        };
        let head = snapshot
            .interned
            .iter()
            .find(|entry| entry.kind == InternKind::Commit && entry.id == head.0 as i64)
            .ok_or(Error::Invalid("tracked head is not interned"))?
            .value
            .clone();
        let known_heads = snapshot
            .interned
            .iter()
            .filter(|entry| entry.kind == InternKind::Commit && entry.scope == row.id)
            .map(|entry| entry.value.clone())
            .collect();
        let approved = approved_head(snapshot, n.approved)?;
        result.push(Tracked {
            number: row.value["number"]
                .as_u64()
                .ok_or(Error::Invalid("invalid PR number"))?,
            missing: open_missing_checks(snapshot, node, &approved)?,
            head,
            approved,
            known_heads,
        });
    }
    Ok(result)
}
fn open_missing_checks(
    snapshot: &Snapshot,
    node: NodeId,
    head: &str,
) -> Result<Vec<RequiredCheck>> {
    let mut open = Vec::new();
    for row in snapshot
        .rows
        .iter()
        .filter(|row| row.table == "queue_facts")
    {
        let Ok(fact) = serde_json::from_value::<QueueFact>(row.value.clone()) else {
            continue;
        };
        if let Evidence::CheckMissing {
            head: missing,
            required,
        } = &fact.evidence
            && route::projection::missing_head(missing == head)
            && fact.node(snapshot)? == Some(node)
        {
            let id = super::looks::missing_check_incident_id(node, head, required);
            if route::projection::missing(
                true,
                true,
                snapshot.rows.iter().any(|row| {
                    row.table == "incidents" && row.id == id && row.value["open"] == true
                }),
            ) {
                open.push(required.clone());
            }
        }
    }
    Ok(open)
}
fn pending_enqueues(snapshot: &Snapshot, repo: &Repository, number: u64) -> Result<Vec<Input>> {
    let mut pending = Vec::new();
    for effect in &snapshot.pending_effects {
        let delivery = super::hydrate(effect, snapshot)?;
        if route::projection::pending(
            delivery.repository == *repo,
            delivery.pull_request == Some(number),
            matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::Enqueue,
                    ..
                }
            ),
        ) {
            pending.push(Input::Look {
                key: effect.key.clone(),
            });
        }
    }
    Ok(pending)
}
fn actor(actor: Option<&Actor>) -> facts::Actor {
    facts::Actor::parse(actor.map(|actor| actor.login.as_str()))
}
fn event_kind(event: &str, label: Option<&str>) -> facts::EventKind {
    match event {
        "unlabeled" => facts::EventKind::Unlabeled {
            queue_label: label == Some(QUEUE_LABEL),
        },
        "closed" => facts::EventKind::Closed,
        "merged" => facts::EventKind::Merged,
        _ => facts::EventKind::Other,
    }
}
fn person_facts(
    repo: &Repository,
    number: u64,
    events: &[tl3_github::IssueActivity],
    merged: bool,
) -> Vec<QueueFact> {
    let events: Vec<_> = events
        .iter()
        .map(|event| facts::IssueEvent {
            id: event.id,
            actor: actor(event.actor.as_ref()),
            kind: event_kind(
                &event.event,
                event.label.as_ref().map(|label| label.name.as_str()),
            ),
        })
        .collect();
    facts::person_facts(&events, merged)
        .map(|fact| QueueFact {
            repository: repo.clone(),
            number,
            evidence: match fact {
                facts::PersonFact::Dequeued { event } => Evidence::DequeuedByPerson { event },
                facts::PersonFact::Closed { event } => Evidence::ClosedByPerson { event },
            },
        })
        .collect()
}
fn route_event(event: &Event) -> route::webhook::Event {
    match event {
        Event::PullRequestMerged { .. } => route::webhook::Event::Merged,
        Event::PullRequestClosed { .. } => route::webhook::Event::Closed,
        Event::PullRequestUnlabeled { .. } => route::webhook::Event::Unlabeled,
        Event::PullRequestSynchronized { .. } => route::webhook::Event::Synchronized,
        Event::CheckRunCompleted { .. } | Event::CheckRunUpdated { .. } => {
            route::webhook::Event::CheckRun
        }
        Event::CheckSuiteCompleted { .. } => route::webhook::Event::CheckSuite,
        Event::MergeGroup { .. } => route::webhook::Event::MergeGroup,
        _ => route::webhook::Event::Other,
    }
}
fn route_check(check: &CheckRun) -> route::webhook::Check {
    route::webhook::check(
        check.name == tl3_mergify::QUEUE_CHECK,
        check
            .app
            .as_ref()
            .is_some_and(|app| app.id == tl3_mergify::MERGIFY_APP_ID),
        check.app.as_ref().is_some_and(|app| app.slug == "mergify"),
    )
}
mod head_checks {
    include!("route-current-head.rs");
}
mod ci_evidence {
    use crate::mergify_io::io::mock_std as std;
    include!("current-ci.rs");
}
pub(super) fn run(
    entry: io::Entry,
    snapshot: &Snapshot,
    event: &Event,
    changes: &RepoChanges,
    repos: &[Repository],
    state: &tl3_mergify::Observation,
    expected: &str,
    open: &[RequiredCheck],
    no_client: bool,
) -> Value {
    let clients = if no_client {
        BTreeMap::new()
    } else {
        [(3, GitHub::new())].into()
    };
    let translator = Translator {
        clients,
        runtime: &RUNTIME,
    };
    let result = match entry {
        io::Entry::Webhook => translator.webhook(event, snapshot, repos).map(|v| json!(v)),
        io::Entry::CatchUp => translator
            .catch_up(&repo(), changes, snapshot)
            .map(|v| json!(v)),
        io::Entry::QueueFacts => RUNTIME
            .block_on(queue_facts(
                &GitHub::new(),
                &repo(),
                state,
                expected,
                open,
                Some(snapshot),
            ))
            .map(|v| json!(v)),
        _ => unreachable!(),
    };
    match result {
        Ok(value) => value,
        Err(error) => json!({"error":error.to_string()}),
    }
}
