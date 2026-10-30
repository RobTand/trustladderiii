use crate::executor::{CommentKind, Delivery, Output};
use crate::{Controller, Error, Repository, Request, Result, put, request, snapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_kernel::{Answer, Command, Event, NodeId, Outcome, Phase, QKind, World};
use tl3_store::{Commit, NewEffect, RowChange, Snapshot};

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
    },
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
}

fn question(kind: QKind) -> (&'static str, Vec<&'static str>) {
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

impl Controller {
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
            let Task::FileRequest {
                text,
                correction: false,
                ..
            } = serde_json::from_value(row.value.clone())?
            else {
                return Err(Error::Invalid("invalid pending replacement"));
            };
            let node = NodeId(index as u64);
            let binding = request(snapshot, node)?;
            commit.rows.push(RowChange::Delete {
                table: "replacement".into(),
                id: row.id.clone(),
            });
            queue_comment(
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
                    question_context: None,
                    request_text: binding.source,
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
            let repository: Repository = serde_json::from_value(row.value["repository"].clone())?;
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
        let open_question = n.question.map(|q| {
            let (kind, answers) = question(q.kind);
            json!({"id": q.id.to_string(), "kind": if q.clarified { "unclear_answer" } else { kind }, "answers": answers})
        });
        let mut inputs = json!({"comment": {"author": row.value["comment"]["user"]["login"], "body": row.value["comment"]["body"]},
                "state": {"phase": format!("{:?}", n.phase), "closed": n.phase.is_closed(), "open_question": open_question}});
        if let Some(pending) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "comment_questions" && r.value["node"] == json!(node))
        {
            inputs["conversation"] = json!({"comment_id": pending.id, "comment": pending.value["comment"], "question": pending.value["question"]});
        }
        let binding: Request = serde_json::from_value(binding.value.clone())?;
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
            question_context: None,
            request_text: binding.source,
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
        queue_classifier(&mut commit, delivery);
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
                    serde_json::from_value::<Delivery>(effect.payload)?.node().0
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
                e.key == format!("clarify:{}", row.id) || e.key == format!("correction:{}", row.id)
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
        let delivery: Delivery = serde_json::from_value(job.value.clone())?;
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
            queue_classifier(commit, retry);
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
        commit
            .rows
            .push(put("github_comments", comment_key.clone(), recorded));
        let kind = reply["kind"].as_str().unwrap();
        if let Some(previous) = inputs["conversation"]["comment_id"].as_str() {
            commit.rows.push(RowChange::Delete {
                table: "comment_questions".into(),
                id: previous.into(),
            });
        }
        if kind == "unsure" {
            let question = reply["question"].as_str().unwrap().to_owned();
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
            queue_comment(commit, clarification);
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
                    reply["summary"].as_str().unwrap(),
                    inputs["comment"]["body"].as_str().unwrap()
                ),
                comment: comment.value["comment"]["id"].as_u64().unwrap(),
                correction: true,
                revert: reply["revert"].as_bool().unwrap(),
                since: comment.value["comment"]["created_at"]
                    .as_str()
                    .unwrap()
                    .into(),
                comment_issue: comment.value["issue"].as_u64().unwrap(),
            });
            queue_comment(commit, correction);
            return Ok(None);
        }
        if n.phase.is_closed() {
            return Ok(None);
        }
        let command = match kind {
            "pause" => Command::Pause,
            "resume" => Command::Resume,
            "stop" => Command::Stop,
            "change_request" => Command::ChangeRequest,
            "remark" => return Ok(None),
            "answer" => Command::Answer {
                question: reply["question_id"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .map_err(|_| Error::Invalid("invalid question id"))?,
                answer: match reply["answer"].as_str().unwrap() {
                    "continue" => Answer::Continue,
                    "unclear" => Answer::Unclear,
                    "change_request" => Answer::ChangeRequest,
                    "stop" => Answer::Stop,
                    "try_again" => Answer::TryAgain,
                    "add_credit" => Answer::AddCredit(reply["amount"].as_u64().unwrap() as u8),
                    "retry" => Answer::Retry,
                    "mark_handled" => Answer::MarkHandled,
                    _ => unreachable!("validated classifier answer"),
                },
            },
            _ => unreachable!("validated classification"),
        };
        let current_answer = match command {
            Command::Answer { question, .. } => n.question.is_some_and(|q| q.id == question),
            _ => true,
        };
        if !current_answer {
            return Ok(None);
        }
        if let Some(text) = reply["new_text"].as_str()
            && !matches!(n.phase, tl3_kernel::Phase::Stopping(_))
        {
            commit.rows.push(put(
                "replacement",
                node.0.to_string(),
                json!(Task::FileRequest {
                    text: text.into(),
                    comment: comment.value["comment"]["id"].as_u64().unwrap(),
                    correction: false,
                    revert: false,
                    since: comment.value["comment"]["created_at"]
                        .as_str()
                        .unwrap()
                        .into(),
                    comment_issue: comment.value["issue"].as_u64().unwrap(),
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
        commit.rows.push(put("replacement_pending", id, value));
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
        if issue.number == old.issue
            && !snapshot
                .rows
                .iter()
                .any(|row| row.table == "replacement_pending" && row.id == original.0.to_string())
        {
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
        };
        let repository = self.intake_repository(&labelled).ok_or(Error::Invalid(
            "linked request is not authorized for intake",
        ))?;
        let event = self.prepare_intake(snapshot, &labelled, repository.clone(), commit)?;
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
        commit.rows.push(put(
            "request_links",
            node.0.to_string(),
            json!({"original": original, "authorized_by": self.bootstrap.rob, "issue": issue}),
        ));
        Ok(event)
    }
}

pub(crate) fn replacement_issue(
    snapshot: &Snapshot,
    original: NodeId,
    value: &Value,
) -> Result<tl3_github::Issue> {
    let Task::FileRequest {
        text,
        since,
        correction: false,
        ..
    } = serde_json::from_value(value.clone())?
    else {
        return Err(Error::Invalid("invalid pending replacement"));
    };
    let old = request(snapshot, original)?;
    Ok(tl3_github::Issue {
        number: old.issue,
        title: old.title,
        body: Some(text),
        state: "open".into(),
        updated_at: since
            .parse()
            .map_err(|_| Error::Invalid("invalid replacement timestamp"))?,
        labels: vec![],
        pull_request: None,
    })
}

fn queue_comment(commit: &mut Commit, delivery: Delivery) {
    commit.effects.push(NewEffect {
        key: delivery.key.clone(),
        kind: "GitHub".into(),
        payload: json!(delivery),
    });
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
                    r.table == "requests" && r.id == pr.id && r.value["repo"] == json!(repository)
                })?
                .value["issue"]
                .as_u64()
        })
        .unwrap_or(issue);
    crate::latest_request(snapshot, repository, issue)
}

fn queue_classifier(commit: &mut Commit, delivery: Delivery) {
    commit
        .rows
        .push(put("comment_jobs", delivery.key.clone(), json!(delivery)));
    commit
        .rows
        .push(put("watches", delivery.key.clone(), json!(delivery)));
    commit.effects.push(NewEffect {
        key: delivery.key.clone(),
        kind: "Workers".into(),
        payload: json!(delivery),
    });
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
