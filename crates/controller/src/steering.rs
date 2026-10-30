use crate::executor::{CommentKind, Delivery, Output};
use crate::{Controller, Error, Repository, Request, Result, put, request, snapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_kernel::{Answer, Command, Event, NodeId, Notice, QKind, World};
use tl3_store::{Commit, NewEffect, RowChange, Snapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Task {
    Classify {
        comment_key: String,
        inputs: Value,
    },
    ClarifyComment {
        question: String,
    },
    FileRequest {
        text: String,
        comment: u64,
        correction: bool,
        revert: bool,
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
    pub(crate) fn schedule_comment(&mut self, snapshot: &Snapshot) -> Result<bool> {
        // One outstanding classification preserves comment order across restarts.
        if snapshot.rows.iter().any(|r| r.table == "comment_jobs") {
            return Ok(false);
        }
        let world = snapshot::decode(snapshot)?;
        let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "unclassified_comments")
        else {
            return Ok(false);
        };
        let repository: Repository = serde_json::from_value(row.value["repository"].clone())?;
        if row.value["comment"]["user"]["id"] != self.bootstrap.rob
            || !self.bootstrap.repos.contains(&repository)
        {
            return self.ignore_comment(snapshot, &row.id);
        }
        let Some(binding) = snapshot.rows.iter().find(|r| {
            r.table == "requests"
                && r.value["repo"] == json!(repository)
                && (r.value["issue"] == row.value["issue"]
                    || snapshot.rows.iter().any(|pr| {
                        pr.table == "pull_requests"
                            && pr.id == r.id
                            && pr.value["number"] == row.value["issue"]
                    }))
        }) else {
            return self.ignore_comment(snapshot, &row.id);
        };
        let node = NodeId(
            binding
                .id
                .parse()
                .map_err(|_| Error::Invalid("invalid request node"))?,
        );
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
            question: n.question,
            note: None,
            question_context: None,
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
        commit
            .rows
            .push(put("comment_jobs", key.clone(), json!(delivery)));
        commit
            .rows
            .push(put("watches", key.clone(), json!(delivery)));
        commit.effects.push(NewEffect {
            key,
            kind: "Workers".into(),
            payload: json!(delivery),
        });
        self.store.commit(&commit)?;
        Ok(true)
    }

    fn ignore_comment(&mut self, snapshot: &Snapshot, id: &str) -> Result<bool> {
        let mut commit = Commit::new(
            format!("ignore-comment:{id}"),
            json!({"comment_ignored": id}),
            snapshot,
        );
        commit.rows.push(RowChange::Delete {
            table: "unclassified_comments".into(),
            id: id.into(),
        });
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "github_comments" && r.id == id)
        {
            let mut value = row.value.clone();
            value["ignored"] = json!(true);
            commit.rows.push(put("github_comments", id.into(), value));
        }
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
        let reply = reply.filter(|r| crate::adapters::workers::protocol::validate_classification(inputs, r).is_ok())
            .unwrap_or(json!({"kind": "unsure", "question": "What would you like me to do with your comment?"}));
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
            clarification.steering = Some(Task::ClarifyComment { question });
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
                text: reply["summary"].as_str().unwrap().into(),
                comment: comment.value["comment"]["id"].as_u64().unwrap(),
                correction: true,
                revert: reply["revert"].as_bool().unwrap(),
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

    pub(crate) fn steering_delivery(
        &self,
        snapshot: &Snapshot,
        commit: &Commit,
        delivery: &mut Delivery,
    ) -> Result<()> {
        let table = match delivery.output {
            Output::Notice {
                notice: Notice::FileReplacement,
                ..
            } => "replacement",
            _ => return Ok(()),
        };
        let id = delivery.node().0.to_string();
        let value = commit
            .rows
            .iter()
            .rev()
            .find_map(|r| match r {
                RowChange::Put {
                    table: t,
                    id: i,
                    value,
                } if t == table && *i == id => Some(value),
                _ => None,
            })
            .or_else(|| {
                snapshot
                    .rows
                    .iter()
                    .find(|r| r.table == table && r.id == id)
                    .map(|r| &r.value)
            })
            .ok_or(Error::Invalid("steering notice context missing"))?;
        delivery.steering = Some(serde_json::from_value(value.clone())?);
        Ok(())
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
        let node = match event {
            Some(_) => NodeId(snapshot::decode(snapshot)?.nodes.len() as u64),
            None => {
                let Some(row) = snapshot.rows.iter().find(|r| {
                    r.table == "requests"
                        && r.value["repo"] == json!(repository)
                        && r.value["issue"] == issue.number
                }) else {
                    return Ok(None);
                };
                NodeId(
                    row.id
                        .parse()
                        .map_err(|_| Error::Invalid("invalid request node"))?,
                )
            }
        };
        commit.rows.push(put(
            "request_links",
            node.0.to_string(),
            json!({"original": original, "authorized_by": self.bootstrap.rob, "issue": issue}),
        ));
        Ok(event)
    }
}

fn queue_comment(commit: &mut Commit, delivery: Delivery) {
    commit.effects.push(NewEffect {
        key: delivery.key.clone(),
        kind: "GitHub".into(),
        payload: json!(delivery),
    });
}
