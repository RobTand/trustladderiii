use crate::executor::{CommentKind, Delivery, Output};
use crate::{Controller, Error, Repository, Request, Result, put, request, snapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_kernel::{Answer, Command, Event, NodeId, Outcome, Phase, QKind, World};
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Task {
    Classify {
        comment_key: String,
        inputs: Value,
    },
    ClarifyComment {
        question: String,
        excerpt: String,
    },
    ReplacementDropped {
        text: String,
        outcome: Outcome,
    },
    FileRequest {
        text: String,
        comment: u64,
        correction: bool,
        revert: bool,
        since: String,
        comment_issue: u64,
        /// #449: the question Rob's answer replied to, for the replacement's ingest input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<Value>,
    },
    /// Q49: a restart files its request from the issue text read when it applies.
    Restart {
        comment_key: String,
        text: String,
        issue: Option<tl3_github::Issue>,
    },
    /// The request's issue is gone, so the restart was dropped and nothing filed.
    RestartDropped {
        text: String,
    },
    /// PRD #327, C3: the audit note for a console command, posted as written.
    /// Q54's operations note uses it too.
    ConsoleNote {
        text: String,
    },
    /// #403: the postmortem a runner claimed, under the claim's run key.
    Postmortem {
        id: i64,
        run_key: String,
    },
}

/// Where a steering instruction came from: Rob's issue comment, or a console
/// command acting as one.
pub(crate) struct Origin<'a> {
    /// Unique per instruction; keys the restart and its effects.
    pub key: &'a str,
    pub text: &'a str,
    /// The GitHub comment, or 0 for a console command.
    pub comment: u64,
    pub since: String,
    pub issue: u64,
}

impl Delivery {
    pub fn is_classifier(&self) -> bool {
        matches!(
            self.output,
            Output::Comment {
                kind: CommentKind::Classify,
                ..
            }
        )
    }

    pub fn is_postmortem(&self) -> bool {
        matches!(
            self.output,
            Output::Comment {
                kind: CommentKind::Postmortem,
                ..
            }
        )
    }

    /// The world table whose row keeps a worker job outside the kernel live.
    pub(crate) fn job_table(&self) -> Option<&'static str> {
        if self.is_classifier() {
            Some("comment_jobs")
        } else if self.is_postmortem() {
            Some(crate::postmortems::JOBS)
        } else {
            None
        }
    }
}

pub(crate) fn question(kind: QKind) -> (&'static str, Vec<&'static str>) {
    match kind {
        QKind::AuditDisagreement => (
            "audit_disagreement",
            vec!["continue", "change_request", "stop"],
        ),
        QKind::NeedsSplitting => ("needs_splitting", vec!["change_request", "stop"]),
        QKind::CantMeet => ("cant_meet", vec!["try_again", "change_request", "stop"]),
        QKind::OutOfCredit => (
            "out_of_credit",
            vec!["add_credit", "change_request", "stop"],
        ),
        QKind::EffectFailed => ("effect_failed", vec!["continue", "stop"]),
        QKind::CleanupFailed => ("cleanup_cant_finish", vec!["retry", "mark_handled"]),
    }
}

fn delegated_comment(
    author: u64,
    rob: u64,
    body: &str,
    configured: &[String],
) -> Option<(String, String)> {
    if author != rob {
        return None;
    }
    let matches: Vec<_> = body
        .lines()
        .enumerate()
        .filter(|(_, line)| configured.iter().any(|entry| entry == line))
        .collect();
    let [(index, attribution)] = matches.as_slice() else {
        return None;
    };
    if tl3_github::message::trailing_plain_line(body) != Some(*attribution) {
        return None;
    }
    let command = body
        .lines()
        .enumerate()
        .filter(|(i, _)| i != index)
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n");
    Some(((*attribution).into(), command.trim().into()))
}

impl Controller {
    pub fn set_delegated_attributions(&mut self, attributions: &[String]) {
        self.delegated_attributions = attributions.to_vec();
    }
    pub(crate) fn close_replacement(
        &self,
        snapshot: &Snapshot,
        previous: &World,
        world: &World,
        commit: &mut Commit,
    ) -> Result<()> {
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "replacement")
        {
            let index: usize = row
                .id
                .parse()
                .map_err(|_| Error::Invalid("invalid request node"))?;
            let Phase::Closed(outcome) = world.nodes[index].phase else {
                continue;
            };
            // FileReplacement already authorizes a new request when Replaced closes.
            if outcome == Outcome::Replaced || previous.nodes[index].phase.is_closed() {
                continue;
            }
            let node = NodeId(index as u64);
            let text = match crate::decode_stored(row.value.clone())? {
                Task::FileRequest {
                    text,
                    correction: false,
                    ..
                } => text,
                // A restart asked for while the stop finished applies once the node closes.
                restart @ Task::Restart { issue: None, .. } if restartable_outcome(outcome) => {
                    self.queue_restart(snapshot, world, node, restart, commit)?;
                    continue;
                }
                Task::Restart { text, .. } => text,
                _ => return Err(Error::Invalid("invalid pending replacement")),
            };
            let binding = request(snapshot, node)?;
            commit.rows.push(RowChange::Delete {
                table: "replacement".into(),
                id: row.id.clone(),
            });
            queue_comment(
                self,
                commit,
                Delivery {
                    key: format!("replacement-dropped:{}", row.id),
                    epoch: snapshot.epoch.get(),
                    output: Output::Comment {
                        node,
                        kind: CommentKind::ReplacementDropped,
                    },
                    generation: world.nodes[index].generation,
                    head_sha: None,
                    repository: binding.repo,
                    issue: binding.issue,
                    pull_request: None,
                    paths: vec![],
                    attempt: 1,
                    round: 0,
                    replanned: world.nodes[index].replanned,
                    checks: None,
                    job_generation: None,
                    question: None,
                    marker_key: None,
                    note: None,
                    scope_removals: None,
                    question_context: None,
                    request_text: binding.source,
                    evidence: None,
                    proposed: None,
                    steering: Some(Task::ReplacementDropped { text, outcome }),
                },
            );
        }
        Ok(())
    }

    pub(crate) fn schedule_comment(&mut self, snapshot: &Snapshot) -> Result<bool> {
        if !snapshot
            .rows
            .iter()
            .any(|r| r.table == "unclassified_comments")
        {
            return Ok(false);
        }
        let world = snapshot::decode(snapshot)?;
        let mut selected = None;
        for row in snapshot
            .rows
            .iter()
            .filter(|r| r.table == "unclassified_comments")
        {
            let repository: Repository = crate::decode_stored(row.value["repository"].clone())?;
            if row.value["comment"]["user"]["id"] != self.bootstrap.rob
                || !self.bootstrap.repos.contains(&repository)
            {
                return self.ignore_comment(snapshot, &row.id);
            }
            let Some(binding) =
                comment_binding(snapshot, &repository, row.value["issue"].as_u64().unwrap())
            else {
                return self.ignore_comment(snapshot, &row.id);
            };
            let node = NodeId(
                binding
                    .id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid request node"))?,
            );
            // Keep comment order within a request while other requests can classify.
            if snapshot.rows.iter().any(|r| {
                r.table == "comment_jobs" && r.value["output"]["Comment"]["node"] == json!(node)
            }) {
                continue;
            }
            selected = Some((row, repository, binding, node));
            break;
        }
        let Some((row, repository, binding, node)) = selected else {
            return Ok(false);
        };
        let n = &world.nodes[node.0 as usize];
        let mut inputs = json!({"comment": {"author": row.value["comment"]["user"]["login"], "body": row.value["comment"]["body"]},
                "state": state(n)});
        let comment: tl3_github::Comment = crate::decode_stored(row.value["comment"].clone())?;
        let delegation = if comment.updated_at > comment.created_at {
            None
        } else {
            delegated_comment(
                comment.user.id,
                self.bootstrap.rob,
                &comment.body,
                &self.delegated_attributions,
            )
        };
        if let Some((attribution, body)) = &delegation {
            inputs["comment"]["body"] = json!(body);
            inputs["comment"]["delegated_by"] = json!(attribution);
        }
        let deterministic_restart = delegation
            .as_ref()
            .is_some_and(|(_, body)| body.eq_ignore_ascii_case("restart"))
            && inputs["state"]["restartable"] == true;
        if let Some(pending) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "comment_questions" && r.value["node"] == json!(node))
        {
            inputs["conversation"] = json!({"comment_id": pending.id, "comment": pending.value["comment"], "question": pending.value["question"]});
        }
        inputs["decision_context"] =
            json!(crate::decisions::Context::load(snapshot, node)?.applicable());
        if let Some(open) = n.question {
            inputs["question_context"] =
                answered_question(snapshot, n.question, open.id).unwrap_or_default();
        }
        let comment_id = row.value["comment"]["id"].to_string();
        if let Some(selected) = snapshot.rows.iter().find(|selected| {
            selected.table == "console_selected_answers" && selected.id == comment_id
        }) {
            inputs["selected_answer"] = selected.value.clone();
        }
        let binding: Request = crate::decode_stored(binding.value.clone())?;
        let key = format!("classify:{}", row.id);
        let delivery = Delivery {
            key: key.clone(),
            epoch: snapshot.epoch.get(),
            output: Output::Comment {
                node,
                kind: CommentKind::Classify,
            },
            generation: n.generation,
            head_sha: None,
            repository,
            issue: binding.issue,
            pull_request: None,
            paths: vec![],
            attempt: 1,
            round: 0,
            replanned: n.replanned,
            checks: None,
            job_generation: None,
            question: n.question,
            marker_key: None,
            note: None,
            scope_removals: None,
            question_context: None,
            request_text: binding.source,
            evidence: None,
            proposed: None,
            steering: Some(Task::Classify {
                comment_key: row.id.clone(),
                inputs,
            }),
        };
        let mut commit = Commit::new(
            format!("schedule:{key}"),
            json!({"classify": row.id}),
            snapshot,
        );
        if let Some((attribution, _)) = &delegation {
            commit.rows.push(put("request_delegations", row.id.clone(), json!({
                "node":node, "comment_key":row.id, "comment_id":row.value["comment"]["id"], "actor_id":self.bootstrap.rob, "attribution":attribution})));
        }
        if deterministic_restart {
            commit
                .rows
                .push(put("comment_jobs", delivery.key.clone(), json!(delivery)));
            self.queue_observation(
                snapshot,
                &mut commit,
                &key,
                0,
                &crate::executor::Observation::Classified {
                    key: delivery.key.clone(),
                    reply: Some(json!({"kind":"restart"})),
                },
            )?;
        } else {
            queue_classifier(self, &mut commit, delivery);
        }
        self.store.commit(&commit)?;
        Ok(true)
    }

    pub(crate) fn retire_comment(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for row in snapshot
            .rows
            .iter()
            .filter(|r| r.table == "github_comments" && r.value.get("classification").is_some())
        {
            let node = match row.value["node"].as_u64() {
                Some(node) => node,
                None => {
                    // Older applied rows predate the node field; their delivery owns the binding.
                    let Some(effect) = self.store.effect(&format!("classify:{}", row.id))? else {
                        continue;
                    };
                    crate::decode_stored::<Delivery>(effect.payload)?.node().0
                }
            };
            let closed = snapshot.rows.iter().any(|r| {
                r.table == "nodes"
                    && r.id == node.to_string()
                    && r.value["phase"].get("Closed").is_some()
            });
            let pending = snapshot.rows.iter().any(|r| {
                (r.table == "unclassified_comments" && r.id == row.id)
                    || (r.table == "comment_jobs"
                        && r.value["steering"]["Classify"]["comment_key"] == row.id)
            }) || snapshot.pending_effects.iter().any(|e| {
                e.key == format!("clarify:{}", row.id)
                    || e.key == format!("correction:{}", row.id)
                    || e.key == format!("restart:{}", row.id)
            });
            if closed && !pending {
                let mut commit = Commit::new(
                    format!("retire-comment:{}", row.id),
                    json!({"comment_retired": row.id}),
                    snapshot,
                );
                for table in ["github_comments", "comment_questions"] {
                    commit.rows.push(RowChange::Delete {
                        table: table.into(),
                        id: row.id.clone(),
                    });
                }
                self.store.commit(&commit)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn ignore_comment(&mut self, snapshot: &Snapshot, id: &str) -> Result<bool> {
        let mut commit = Commit::new(
            format!("retire-comment:{id}"),
            json!({"comment_ignored": id}),
            snapshot,
        );
        commit.rows.push(RowChange::Delete {
            table: "unclassified_comments".into(),
            id: id.into(),
        });
        commit.rows.push(RowChange::Delete {
            table: "github_comments".into(),
            id: id.into(),
        });
        self.store.commit(&commit)?;
        Ok(true)
    }

    pub(crate) fn classified(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        key: &str,
        reply: Option<Value>,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let Some(job) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "comment_jobs" && r.id == key)
        else {
            return Ok(None);
        };
        let delivery: Delivery = crate::decode_stored(job.value.clone())?;
        let Some(Task::Classify {
            comment_key,
            inputs,
        }) = &delivery.steering
        else {
            return Err(Error::Invalid("invalid classifier job"));
        };
        let node = delivery.node();
        let n = &world.nodes[node.0 as usize];
        let reply = reply.filter(|r| {
            crate::adapters::workers::protocol::validate_classification(inputs, r).is_ok()
        });
        if reply.is_none() && delivery.attempt < 3 {
            let mut retry = delivery.clone();
            retry.attempt += 1;
            retry.key = format!("classify:{comment_key}:attempt:{}", retry.attempt);
            commit.rows.push(RowChange::Delete {
                table: "comment_jobs".into(),
                id: key.into(),
            });
            commit.rows.push(RowChange::Delete {
                table: "watches".into(),
                id: key.into(),
            });
            queue_classifier(self, commit, retry);
            return Ok(None);
        }
        if inputs.get("reconcile_issue").is_some() {
            for table in ["comment_jobs", "watches"] {
                commit.rows.push(RowChange::Delete {
                    table: table.into(),
                    id: key.into(),
                });
            }
            let mut context = crate::decisions::Context::load(snapshot, node)?;
            let issue: tl3_github::Issue = crate::decode_stored(inputs["reconcile_issue"].clone())?;
            let source = format!("issue:{}@{}", issue.number, issue.updated_at);
            for decision in &mut context.decisions {
                let assessment = reply
                    .as_ref()
                    .and_then(|r| r["assessments"].as_array())
                    .and_then(|items| items.iter().find(|a| a["id"] == decision.id));
                // An unavailable assessment cannot authorize an exception. The
                // restarted worker can ask for renewed approval from current text.
                if assessment.is_none_or(|a| a["contradicted"] == true) {
                    decision.superseded_by = Some(source.clone());
                    decision.supersession_reason = Some(
                        assessment
                            .and_then(|a| a["reason"].as_str())
                            .unwrap_or(
                                "Current issue reconciliation failed; renewed approval is required",
                            )
                            .into(),
                    );
                }
            }
            context.store(snapshot, commit)?;
            let pending = snapshot
                .rows
                .iter()
                .find(|r| r.table == "restart_reconciliation" && r.id == node.0.to_string())
                .ok_or(Error::Invalid("restart reconciliation missing"))?;
            commit.rows.push(put(
                "replacement_pending",
                pending.id.clone(),
                pending.value.clone(),
            ));
            commit.rows.push(RowChange::Delete {
                table: "restart_reconciliation".into(),
                id: pending.id.clone(),
            });
            return Ok(None);
        }
        let reply = reply.unwrap_or(json!({"kind": "unsure", "question": "I couldn't act on your comment. Could you rephrase it?"}));
        for (table, id) in [
            ("comment_jobs", key),
            ("watches", key),
            ("unclassified_comments", comment_key),
        ] {
            commit.rows.push(RowChange::Delete {
                table: table.into(),
                id: id.into(),
            });
        }
        // The stored comment is the durable deduplication record in both ingress paths.
        let comment = snapshot
            .rows
            .iter()
            .find(|r| r.table == "github_comments" && r.id == *comment_key)
            .ok_or(Error::Invalid("classifier comment missing"))?;
        let mut recorded = comment.value.clone();
        recorded["classification"] = reply.clone();
        recorded["node"] = json!(node);
        if let Some(delegate) = inputs["comment"].get("delegated_by") {
            recorded["delegated_by"] = delegate.clone();
        }
        commit
            .rows
            .push(put("github_comments", comment_key.clone(), recorded));
        if inputs.get("selected_answer").is_some() {
            commit.rows.push(RowChange::Delete {
                table: "console_selected_answers".into(),
                id: comment.value["comment"]["id"].to_string(),
            });
        }
        let kind: String = crate::adapters::workers::protocol::reply_field(&reply, "kind")?;
        if let Some(previous) = inputs["conversation"]["comment_id"].as_str() {
            commit.rows.push(RowChange::Delete {
                table: "comment_questions".into(),
                id: previous.into(),
            });
        }
        if kind == "unsure" {
            let question = crate::adapters::workers::protocol::reply_field(&reply, "question")?;
            commit.rows.push(put(
                "comment_questions",
                comment_key.clone(),
                json!({"node": node, "comment": inputs["comment"]["body"], "question": question}),
            ));
            let mut clarification = delivery.clone();
            clarification.key = format!("clarify:{comment_key}");
            clarification.output = Output::Comment {
                node,
                kind: CommentKind::Clarify,
            };
            clarification.steering = Some(Task::ClarifyComment {
                question,
                excerpt: excerpt(inputs["comment"]["body"].as_str().unwrap()),
            });
            queue_comment(self, commit, clarification);
            return Ok(None);
        }
        if kind == "complaint" && n.phase.is_closed() {
            let mut correction = delivery.clone();
            correction.key = format!("correction:{comment_key}");
            correction.output = Output::Comment {
                node,
                kind: CommentKind::FileCorrection,
            };
            correction.steering = Some(Task::FileRequest {
                text: format!(
                    "{}\n\nRob's comment:\n{}",
                    crate::adapters::workers::protocol::reply_field::<String>(&reply, "summary")?,
                    inputs["comment"]["body"].as_str().unwrap()
                ),
                comment: comment.value["comment"]["id"].as_u64().unwrap(),
                correction: true,
                revert: crate::adapters::workers::protocol::reply_field(&reply, "revert")?,
                since: comment.value["comment"]["created_at"]
                    .as_str()
                    .unwrap()
                    .into(),
                comment_issue: comment.value["issue"].as_u64().unwrap(),
                question: None,
            });
            queue_comment(self, commit, correction);
            return Ok(None);
        }
        let origin = Origin {
            key: comment_key,
            text: inputs["comment"]["body"].as_str().unwrap(),
            comment: comment.value["comment"]["id"].as_u64().unwrap(),
            since: comment.value["comment"]["created_at"]
                .as_str()
                .unwrap()
                .into(),
            issue: comment.value["issue"].as_u64().unwrap(),
        };
        self.steer(snapshot, world, node, &reply, &origin, commit)
    }

    /// Apply one classified instruction, from a comment or the console, as Rob.
    pub(crate) fn steer(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        node: NodeId,
        reply: &Value,
        origin: &Origin<'_>,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let n = &world.nodes[node.0 as usize];
        let kind: String = crate::adapters::workers::protocol::reply_field(reply, "kind")?;
        if kind == "restart" {
            // The phase can change while the classifier runs, for example by a merge.
            if !restartable(n.phase) {
                return Ok(None);
            }
            let restart = Task::Restart {
                comment_key: origin.key.into(),
                text: origin.text.into(),
                issue: None,
            };
            commit
                .rows
                .push(put("replacement", node.0.to_string(), json!(restart)));
            return Ok(match n.phase {
                Phase::Closed(_) => {
                    self.queue_restart(snapshot, world, node, restart, commit)?;
                    None
                }
                // Stopping already tears the work down; close_replacement starts it again.
                Phase::Stopping(_) => None,
                _ => Some(Event::Command {
                    node,
                    id: n.last_cmd + 1,
                    command: Command::ChangeRequest,
                }),
            });
        }
        if n.phase.is_closed() {
            return Ok(None);
        }
        let command = match kind.as_str() {
            "pause" => Command::Pause,
            "resume" => Command::Resume,
            "stop" => Command::Stop,
            "change_request" => Command::ChangeRequest,
            "remark" => return Ok(None),
            "answer" => Command::Answer {
                question: crate::adapters::workers::protocol::reply_field::<String>(
                    reply,
                    "question_id",
                )?
                .parse()
                .map_err(|_| Error::Invalid("invalid question id"))?,
                answer: match crate::adapters::workers::protocol::reply_field::<String>(
                    reply, "answer",
                )?
                .as_str()
                {
                    "continue" => Answer::Continue,
                    "unclear" => Answer::Unclear,
                    "change_request" => Answer::ChangeRequest,
                    "stop" => Answer::Stop,
                    "try_again" => Answer::TryAgain,
                    "add_credit" => Answer::AddCredit(credit_amount(reply)?),
                    "retry" => Answer::Retry,
                    "mark_handled" => Answer::MarkHandled,
                    _ => return Err(Error::Invalid("invalid classifier answer")),
                },
            },
            _ => return Err(Error::Invalid("invalid classifier kind")),
        };
        let current_answer = match command {
            Command::Answer { question, .. } => n.question.is_some_and(|q| q.id == question),
            _ => true,
        };
        if !current_answer {
            return Ok(None);
        }
        let changes_request = !matches!(n.phase, Phase::Stopping(_))
            && matches!(
                command,
                Command::ChangeRequest
                    | Command::Answer {
                        answer: Answer::ChangeRequest,
                        ..
                    }
            );
        let accepted_answer =
            matches!(command, Command::Answer { answer, .. } if !matches!(answer, Answer::Unclear));
        if changes_request || accepted_answer {
            let binding = request(snapshot, node)?;
            let source = (origin.comment != 0 && origin.comment < (1u64 << 53)).then(|| {
                format!(
                    "https://github.com/{}/{}/issues/{}#issuecomment-{}",
                    binding.repo.owner, binding.repo.name, origin.issue, origin.comment
                )
            });
            let mut context = crate::decisions::Context::load(snapshot, node)?;
            if changes_request {
                for id in reply["supersedes"].as_array().into_iter().flatten() {
                    let decision = context
                        .decisions
                        .iter_mut()
                        .find(|decision| {
                            decision.superseded_by.is_none()
                                && id.as_str() == Some(decision.id.as_str())
                        })
                        .ok_or(Error::Invalid(
                            "superseded decision is no longer applicable",
                        ))?;
                    decision.superseded_by =
                        Some(source.clone().unwrap_or_else(|| origin.key.into()));
                    decision.supersession_reason = Some(format!(
                        "Replacement request contradicts this approval: {}",
                        reply["new_text"].as_str().unwrap_or(origin.text)
                    ));
                }
            }
            if let Command::Answer { question, .. } = command
                && accepted_answer
            {
                let scope = reply["decision"]["allowed_scope"]
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        reply["new_text"]
                            .as_str()
                            .or_else(|| reply["guidance"].as_str())
                            .map(str::to_owned)
                            .into_iter()
                            .collect()
                    });
                let restrictions = reply["decision"]["restrictions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                context.decisions.push(crate::decisions::Decision {
                    id: origin.key.into(),
                    source_comment: source,
                    question: answered_question(snapshot, n.question, question)
                        .ok_or(Error::Invalid("accepted answer has no question"))?,
                    answer: origin.text.into(),
                    allowed_scope: scope,
                    restrictions,
                    origin_request_version: node.0,
                    superseded_by: None,
                    supersession_reason: None,
                });
            }
            context.store(snapshot, commit)?;
        }
        // #422: Rob's answer, with the question and his reason, is a decision later
        // requests recall. An unclear answer decides nothing.
        if let Command::Answer { question, answer } = &command
            && !matches!(answer, Answer::Unclear)
            && let Some(repository) = crate::stored_repositories(snapshot)?.get(n.repo.0 as usize)
        {
            let context = self.store.question_context(*question)?.unwrap_or_default();
            let asked = context["findings"]["decision"]
                .as_str()
                .unwrap_or("A question TL3 asked about this request.");
            crate::memory::queue_lesson(
                &mut self.store,
                &crate::memory::Lesson {
                    kind: crate::memory::LessonKind::Decision,
                    source: "rob",
                    repo: &format!("{}/{}", repository.owner, repository.name),
                    topic: None,
                    text: &crate::memory::decision(asked, reply, origin.text),
                    identity: origin.key,
                },
            )?;
        }
        if let Some(text) = reply["new_text"].as_str()
            && !matches!(n.phase, tl3_kernel::Phase::Stopping(_))
        {
            commit.rows.push(put(
                "replacement",
                node.0.to_string(),
                json!(Task::FileRequest {
                    text: text.into(),
                    comment: origin.comment,
                    correction: false,
                    revert: false,
                    since: origin.since.clone(),
                    comment_issue: origin.issue,
                    question: match command {
                        Command::Answer { question, .. } => {
                            answered_question(snapshot, n.question, question)
                        }
                        _ => None,
                    },
                }),
            ));
        }
        if reply["answer"] == "try_again" {
            let mut findings = self
                .store
                .worker_document(&node.0.to_string(), "findings")?
                .unwrap_or(json!([]));
            findings
                .as_array_mut()
                .ok_or(Error::Invalid("invalid findings"))?
                .push(reply["guidance"].clone());
            commit
                .rows
                .push(put("findings", node.0.to_string(), findings));
        }
        Ok(Some(Event::Command {
            node,
            id: n.last_cmd + 1,
            command,
        }))
    }

    pub(crate) fn pending_replacement(
        &self,
        snapshot: &Snapshot,
        world: &World,
        commit: &mut Commit,
        node: NodeId,
    ) -> Result<()> {
        let id = node.0.to_string();
        let value = commit
            .rows
            .iter()
            .rev()
            .find_map(|row| match row {
                RowChange::Put {
                    table,
                    id: key,
                    value,
                } if table == "replacement" && *key == id => Some(value.clone()),
                _ => None,
            })
            .or_else(|| {
                snapshot
                    .rows
                    .iter()
                    .find(|row| row.table == "replacement" && row.id == id)
                    .map(|row| row.value.clone())
            })
            .ok_or(Error::Invalid("steering notice context missing"))?;
        // A restart reads the issue first; its observation fills in replacement_pending.
        if let restart @ Task::Restart { .. } = crate::decode_stored(value.clone())? {
            return self.queue_restart(snapshot, world, node, restart, commit);
        }
        commit.rows.push(put("replacement_pending", id, value));
        Ok(())
    }

    fn queue_restart(
        &self,
        snapshot: &Snapshot,
        world: &World,
        node: NodeId,
        restart: Task,
        commit: &mut Commit,
    ) -> Result<()> {
        let Task::Restart { comment_key, .. } = &restart else {
            return Err(Error::Invalid("invalid restart"));
        };
        let binding = request(snapshot, node)?;
        let n = &world.nodes[node.0 as usize];
        queue_comment(
            self,
            commit,
            Delivery {
                key: format!("restart:{comment_key}"),
                epoch: snapshot.epoch.get(),
                output: Output::Comment {
                    node,
                    kind: CommentKind::Restart,
                },
                generation: n.generation,
                head_sha: None,
                repository: binding.repo,
                issue: binding.issue,
                pull_request: None,
                paths: vec![],
                attempt: 1,
                round: 0,
                replanned: n.replanned,
                checks: None,
                job_generation: None,
                question: None,
                marker_key: None,
                note: None,
                scope_removals: None,
                question_context: None,
                request_text: binding.source,
                evidence: None,
                proposed: None,
                steering: Some(restart),
            },
        );
        Ok(())
    }

    /// Q49: a gone issue (404 or 410) ends the restart. Other read failures keep
    /// it pending behind their incident, and clearing that incident retries it.
    pub(crate) fn drop_restart(
        &self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        commit: &mut Commit,
    ) -> Result<()> {
        let Some(Task::Restart { text, .. }) = &delivery.steering else {
            return Err(Error::Invalid("invalid restart"));
        };
        let node = delivery.node();
        let id = node.0.to_string();
        for table in ["replacement", "replacement_pending"] {
            if snapshot.rows.iter().any(|r| r.table == table && r.id == id) {
                commit.rows.push(RowChange::Delete {
                    table: table.into(),
                    id: id.clone(),
                });
            }
        }
        let binding = request(snapshot, node)?;
        let mut note = delivery.clone();
        note.key = format!("restart-dropped:{}", delivery.key);
        note.marker_key = None;
        note.epoch = snapshot.epoch.get();
        note.output = Output::Comment {
            node,
            kind: CommentKind::ReplacementDropped,
        };
        note.question_context = Some(json!({"issue_title": binding.title,
            "request": format!("{} (https://github.com/{}/{}/issues/{})", binding.title,
                binding.repo.owner, binding.repo.name, binding.issue)}));
        note.steering = Some(Task::RestartDropped { text: text.clone() });
        // The request's own issue is gone, so the note goes to the operations issue.
        if let Some(operations) = &self.bootstrap.operations {
            note.repository = Repository {
                owner: operations.owner.clone(),
                name: operations.name.clone(),
                installation: operations.installation_id,
            };
            note.issue = operations.number;
        }
        queue_comment(self, commit, note);
        Ok(())
    }

    pub(crate) fn start_replacement(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for pending in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "replacement_pending")
        {
            let original = NodeId(
                pending
                    .id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid request node"))?,
            );
            let old = request(snapshot, original)?;
            if crate::latest_request(snapshot, &old.repo, old.issue)
                .is_none_or(|latest| latest.id != pending.id)
            {
                continue;
            }
            let mut commit = Commit::new(
                format!("start-replacement:{}", pending.id),
                json!({"replacement": original}),
                snapshot,
            );
            let issue = replacement_issue(snapshot, original, &pending.value)?;
            if let Some(event) =
                self.linked_request(snapshot, old.repo, issue, original, &mut commit)?
            {
                self.apply(snapshot, snapshot::decode(snapshot)?, event, &mut commit)?;
            }
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn linked_request(
        &mut self,
        snapshot: &Snapshot,
        repository: Repository,
        issue: tl3_github::Issue,
        original: NodeId,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let old = request(snapshot, original)?;
        if old.repo != repository {
            return Err(Error::Invalid("linked request repository mismatch"));
        }
        let id = original.0.to_string();
        if issue.number == old.issue
            && !snapshot
                .rows
                .iter()
                .any(|row| row.table == "replacement_pending" && row.id == id)
        {
            // Q49: the restart's new request starts from the issue text read just now.
            if let Some(row) = snapshot
                .rows
                .iter()
                .find(|row| row.table == "replacement" && row.id == id)
                && let Task::Restart {
                    comment_key,
                    text,
                    issue: None,
                } = crate::decode_stored(row.value.clone())?
            {
                let context = crate::decisions::Context::load(snapshot, original)?.applicable();
                let restart = json!(Task::Restart {
                    comment_key: comment_key.clone(),
                    text: text.clone(),
                    issue: Some(issue.clone())
                });
                if context.decisions.is_empty() {
                    commit.rows.push(put("replacement_pending", id, restart));
                } else if !snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "restart_reconciliation" && r.id == id)
                {
                    commit.rows.push(put("restart_reconciliation", id, restart));
                    let n = &snapshot::decode(snapshot)?.nodes[original.0 as usize];
                    let binding = request(snapshot, original)?;
                    queue_classifier(
                        self,
                        commit,
                        Delivery {
                            key: format!("restart-decisions:{comment_key}"),
                            epoch: snapshot.epoch.get(),
                            output: Output::Comment {
                                node: original,
                                kind: CommentKind::Classify,
                            },
                            generation: n.generation,
                            head_sha: None,
                            repository: binding.repo,
                            issue: binding.issue,
                            pull_request: None,
                            paths: vec![],
                            attempt: 1,
                            round: 0,
                            replanned: n.replanned,
                            checks: None,
                            evidence: None,
                            job_generation: None,
                            question: None,
                            marker_key: None,
                            note: None,
                            scope_removals: None,
                            question_context: None,
                            request_text: binding.source,
                            proposed: None,
                            steering: Some(Task::Classify {
                                comment_key: format!("restart-decisions:{comment_key}"),
                                inputs: json!({"comment":{"author":"requester", "body":text}, "state":state(n),
                                "decision_context":context, "reconcile_issue":issue}),
                            }),
                        },
                    );
                }
            }
            return Ok(None);
        }
        let labelled = crate::LabelledIssue {
            delivery: commit.event_key.clone(),
            owner: repository.owner.clone(),
            repo: repository.name.clone(),
            installation: repository.installation,
            number: issue.number,
            title: issue.title.clone(),
            actor: self.bootstrap.rob,
            label: "tl3".into(),
            labels: Vec::new(),
        };
        let repository = self.intake_repository(&labelled).ok_or(Error::Invalid(
            "linked request is not authorized for intake",
        ))?;
        let event = self.prepare_intake(snapshot, &labelled, repository.clone(), None, commit)?;
        // Intake owns the saved text and link for same-issue replacements.
        if issue.number == old.issue {
            return Ok(event);
        }
        let node = match event {
            Some(_) => NodeId(snapshot::decode(snapshot)?.nodes.len() as u64),
            None => {
                let Some(row) = crate::latest_request(snapshot, &repository, issue.number) else {
                    return Ok(None);
                };
                NodeId(
                    row.id
                        .parse()
                        .map_err(|_| Error::Invalid("invalid request node"))?,
                )
            }
        };
        let source = json!(issue.body);
        if let Some(RowChange::Put { value, .. }) = commit.rows.iter_mut().find(|row| {
            matches!(row, RowChange::Put { table, id, .. } if table == "requests" && *id == node.0.to_string())
        }) {
            value["source"] = source;
        } else {
            let mut binding = request(snapshot, node)?;
            binding.source = issue.body.clone();
            commit.rows.push(put("requests", node.0.to_string(), json!(binding)));
        }
        let mut link =
            json!({"original": original, "authorized_by": self.bootstrap.rob, "issue": issue});
        if let Some(delegate) = replacement_delegate(snapshot, original) {
            link["delegated_by"] = delegate.clone();
        }
        commit
            .rows
            .push(put("request_links", node.0.to_string(), link));
        Ok(event)
    }
}

pub(crate) fn replacement_delegate(snapshot: &Snapshot, original: NodeId) -> Option<&Value> {
    let pending = snapshot.rows.iter().find(|r| {
        r.id == original.0.to_string()
            && matches!(r.table.as_str(), "replacement_pending" | "replacement")
    })?;
    let key = pending.value["Restart"]["comment_key"].as_str();
    let comment = pending.value["FileRequest"]["comment"].as_u64();
    snapshot
        .rows
        .iter()
        .find(|r| {
            r.table == "request_delegations"
                && (key.is_some_and(|key| r.id == key)
                    || comment.is_some_and(|id| r.value["comment_id"] == id))
        })
        .map(|r| &r.value["attribution"])
}

pub(crate) fn replacement_issue(
    snapshot: &Snapshot,
    original: NodeId,
    value: &Value,
) -> Result<tl3_github::Issue> {
    let (text, since) = match crate::decode_stored(value.clone())? {
        Task::FileRequest {
            text,
            since,
            correction: false,
            ..
        } => (text, since),
        Task::Restart {
            issue: Some(issue), ..
        } => return Ok(issue),
        _ => return Err(Error::Invalid("invalid pending replacement")),
    };
    let old = request(snapshot, original)?;
    Ok(tl3_github::Issue {
        number: old.issue,
        title: old.title,
        body: Some(text),
        state: "open".into(),
        state_reason: None,
        updated_at: since
            .parse()
            .map_err(|_| Error::Invalid("invalid replacement timestamp"))?,
        labels: vec![],
        pull_request: None,
    })
}

/// #449: what the ingest agent is told about the question an answer replied to.
fn answered_question(
    snapshot: &Snapshot,
    open: Option<tl3_kernel::Question>,
    answered: u64,
) -> Option<Value> {
    let open = open.filter(|open| open.id == answered)?;
    let context = snapshot
        .rows
        .iter()
        .find(|row| row.table == "question_context" && row.id == answered.to_string())
        .map(|row| &row.value);
    let mut question = json!({"id": open.id, "kind": question(open.kind).0});
    for field in ["pieces", "findings"] {
        if let Some(value) = context
            .and_then(|context| context.get(field))
            .filter(|v| !v.is_null())
        {
            question[field] = value.clone();
        }
    }
    Some(question)
}

/// The request state the classifier reads, and that console commands are checked against.
pub(crate) fn state(n: &tl3_kernel::Node) -> Value {
    let open_question = n.question.map(|q| {
        let (kind, answers) = question(q.kind);
        json!({"id": q.id.to_string(), "kind": if q.clarified { "unclear_answer" } else { kind }, "answers": answers})
    });
    json!({"phase": format!("{:?}", n.phase), "closed": n.phase.is_closed(), "open_question": open_question,
        "restartable": restartable(n.phase)})
}

/// Q49: restart is offered until the work merges or is already being replaced.
fn restartable(phase: Phase) -> bool {
    match phase {
        Phase::Stopping(outcome) | Phase::Closed(outcome) => restartable_outcome(outcome),
        _ => true,
    }
}

fn restartable_outcome(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Stopped | Outcome::Duplicate | Outcome::AlreadyDone | Outcome::NotActionable
    )
}

fn queue_comment(controller: &Controller, commit: &mut Commit, delivery: Delivery) {
    commit.rows.push(controller.allowance_row(delivery.clone()));
    commit.effects.push(delivery.into_effect());
}

fn comment_binding<'a>(
    snapshot: &'a Snapshot,
    repository: &Repository,
    issue: u64,
) -> Option<&'a tl3_store::WorldRow> {
    let issue = snapshot
        .rows
        .iter()
        .find_map(|pr| {
            if pr.table != "pull_requests" || pr.value["number"] != issue {
                return None;
            }
            snapshot
                .rows
                .iter()
                .find(|r| {
                    r.table == "requests"
                        && r.id == pr.id
                        && serde_json::from_value::<Repository>(r.value["repo"].clone())
                            .is_ok_and(|stored| stored == *repository)
                })?
                .value["issue"]
                .as_u64()
        })
        .unwrap_or(issue);
    crate::latest_request(snapshot, repository, issue)
}

fn queue_classifier(controller: &Controller, commit: &mut Commit, delivery: Delivery) {
    commit.rows.push(controller.allowance_row(delivery.clone()));
    commit
        .rows
        .push(put("comment_jobs", delivery.key.clone(), json!(delivery)));
    commit
        .rows
        .push(put("watches", delivery.key.clone(), json!(delivery)));
    commit.effects.push(delivery.into_effect());
}

fn excerpt(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .map(|c| if c == '?' { ' ' } else { c })
        .collect()
}

fn credit_amount(reply: &Value) -> Result<u8> {
    crate::adapters::workers::protocol::reply_field(reply, "amount")
}

#[cfg(test)]
mod reply_defects {
    use super::*;
    #[test]
    fn credit_is_checked_without_panics_or_truncation() {
        for amount in [
            Value::Null,
            json!("1"),
            json!(-1),
            json!(1.5),
            json!(256),
            json!(u64::MAX),
        ] {
            assert!(matches!(
                credit_amount(&json!({"amount":amount})),
                Err(Error::Invalid(_))
            ));
        }
        assert_eq!(credit_amount(&json!({"amount":100})).unwrap(), 100);
    }
}

#[cfg(test)]
mod delegation_tests {
    use super::*;
    const LINE: &str = "(Posted by the TL3 build coordinator under Rob's standing approval.)";

    #[test]
    fn exact_attributed_restart_is_deterministic_and_preserves_delegate() {
        let configured = vec![LINE.into()];
        for _ in 0..9 {
            let (delegate, body) =
                delegated_comment(42, 42, &format!("restart\n\n{LINE}"), &configured).unwrap();
            assert_eq!(delegate, LINE);
            assert_eq!(body, "restart");
        }
        for command in [
            "continue",
            "Return goodbye instead.",
            "pause",
            "resume",
            "stop",
        ] {
            assert_eq!(
                delegated_comment(42, 42, &format!("{command}\n\n{LINE}"), &configured)
                    .unwrap()
                    .1,
                command
            );
        }
    }

    #[test]
    fn attribution_requires_the_rob_account_and_one_exact_configured_line() {
        let configured = vec![LINE.into()];
        assert!(delegated_comment(7, 42, &format!("restart\n\n{LINE}"), &configured).is_none());
        for body in [
            format!("restart\n\n{LINE} "),
            format!("restart\n\n>{LINE}"),
            format!("restart\n\n{}", LINE.replace("coordinator", "co-ordinator")),
            format!("restart\n{LINE}\n{LINE}"),
            "restart".into(),
        ] {
            assert!(
                delegated_comment(42, 42, &body, &configured).is_none(),
                "{body}"
            );
        }
        assert!(delegated_comment(42, 42, &format!("restart\n{LINE}"), &[]).is_none());
        assert!(
            delegated_comment(
                42,
                42,
                &format!("restart\n{LINE}\n(Another delegate.)"),
                &[LINE.into(), "(Another delegate.)".into()]
            )
            .is_none()
        );
    }

    #[test]
    fn attribution_must_be_trailing_plain_markdown() {
        for body in [
            format!("restart\n```\n{LINE}\n```"),
            format!("restart\n~~~\n{LINE}\n~~~"),
            format!("restart\n\n```\n{LINE}"),
            format!("restart\n\n~~~\n{LINE}"),
            format!("restart\n\n    {LINE}"),
            format!("restart\n\n\t{LINE}"),
            format!("restart\n\n> {LINE}"),
            format!("restart\n\n> quoted attribution\n{LINE}"),
            format!("restart\n\n<!--\n{LINE}\n-->"),
            format!("restart\n\n<!--\n{LINE}"),
            format!("restart\n\n<div>\n{LINE}\n</div>"),
            format!("restart\n\n{LINE}\nmore text"),
        ] {
            assert!(
                delegated_comment(42, 42, &body, &[LINE.into()]).is_none(),
                "Markdown content must not grant delegation: {body}"
            );
        }
        for body in [
            format!("restart\n{LINE}"),
            format!("restart\n\n{LINE}\n\n"),
            format!("restart\n\n> a quote\n\n{LINE}"),
            format!("restart\n\n```\nexample\n```\n\n{LINE}"),
        ] {
            assert!(delegated_comment(42, 42, &body, &[LINE.into()]).is_some());
        }
    }
}
