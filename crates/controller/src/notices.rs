use crate::executor::{CommentKind, Delivery, Output};
use crate::looks::Incident;
use crate::{Controller, Result, put, request, snapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tl3_kernel::{Answer, Command, Event, Notice, World};
use tl3_store::{Commit, NewEffect, RowChange, Snapshot};

#[derive(Serialize, Deserialize)]
struct FailedNotice {
    delivery: Delivery,
    incident: String,
}

fn kind(delivery: &Delivery) -> &'static str {
    match delivery.output {
        Output::Notice { notice, .. } => match notice {
            Notice::Ask { .. } => "question",
            Notice::Summary(_) => "closing summary",
            Notice::FileReplacement => "replacement filing",
            Notice::Incident { .. } => "incident post",
            Notice::MeasuresPosted | Notice::Note(_) => "note",
        },
        Output::Comment { kind, .. } => match kind {
            CommentKind::Clarify => "clarification question",
            CommentKind::FileCorrection => "replacement filing",
            CommentKind::ReplacementDropped => "dropped-change note",
            CommentKind::Classify => "classification",
        },
        Output::Effect { .. } => unreachable!(),
    }
}

impl Controller {
    pub(crate) fn failed_notice(
        &mut self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        class: &str,
        commit: &mut Commit,
    ) -> Result<()> {
        if matches!(
            delivery.output,
            Output::Notice {
                notice: Notice::Incident { .. },
                ..
            }
        ) {
            // Reporting a reporting failure must stop at the status page.
            for row in snapshot
                .rows
                .iter()
                .filter(|r| r.table == "incidents" && r.value["post_key"] == delivery.key)
            {
                let mut incident: Incident = serde_json::from_value(row.value.clone())?;
                incident.open = true;
                incident.summary.push_str(&format!(
                    " The incident post failed, with class {}.",
                    class.replace('_', " ")
                ));
                commit
                    .rows
                    .push(put("incidents", row.id.clone(), json!(incident)));
            }
            return Ok(());
        }
        let node = delivery.node();
        if let Output::Notice {
            notice: Notice::Ask { question },
            ..
        } = delivery.output
        {
            self.apply(
                snapshot,
                snapshot::decode(snapshot)?,
                Event::NoticeFailed { node, question },
                commit,
            )?;
        }
        let request = request(snapshot, node)?;
        let id = format!("post.{}", snapshot.revision.get() + 1);
        let signature = format!("GitHub:{class}");
        let summary = format!(
            "{} The {} could not be posted. Failure class {}.",
            crate::looks::incident_summary(&request.title, &signature, true),
            kind(delivery),
            class.replace('_', " ")
        );
        let mut post = delivery.clone();
        post.key = format!("incident:{id}");
        post.marker_key = None;
        post.epoch = snapshot.epoch.get();
        post.output = Output::Notice {
            node,
            notice: Notice::Incident { needs_rob: true },
        };
        post.steering = None;
        post.note = Some(format!(
            "{summary} Request: https://github.com/{}/{}/issues/{}. Reply with \"clear incident {id}\" on this issue after fixing the cause.",
            request.repo.owner, request.repo.name, request.issue
        ));
        post.question_context = Some(json!({"issue_title":request.title,
            "request":format!("{} (https://github.com/{}/{}/issues/{})", request.title, request.repo.owner, request.repo.name, request.issue),
            "signature":signature}));
        if let Some(operations) = &self.bootstrap.operations {
            post.repository = crate::Repository {
                owner: operations.owner.clone(),
                name: operations.name.clone(),
                installation: operations.installation_id,
            };
            post.issue = operations.number;
        }
        commit.rows.push(put(
            "incidents",
            id.clone(),
            json!(Incident {
                id: id.clone(),
                node,
                open: true,
                needs_rob: true,
                blocking: false,
                summary,
                signature,
                effect: Some(delivery.key.clone()),
                post_key: Some(post.key.clone()),
            }),
        ));
        commit.rows.push(put(
            "notice_failures",
            delivery.key.clone(),
            json!(FailedNotice {
                delivery: delivery.clone(),
                incident: id
            }),
        ));
        commit.rows.push(self.allowance_row(post.clone()));
        commit.effects.push(NewEffect {
            key: post.key.clone(),
            kind: "GitHub".into(),
            payload: json!(post),
        });
        Ok(())
    }

    pub(crate) fn retry_notice(
        &mut self,
        snapshot: &Snapshot,
        incident: &Incident,
        commit: &mut Commit,
    ) -> Result<bool> {
        let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "notice_failures" && Some(&r.id) == incident.effect.as_ref())
        else {
            return Ok(false);
        };
        let failed: FailedNotice = serde_json::from_value(row.value.clone())?;
        let world = snapshot::decode(snapshot)?;
        let node = &world.nodes[failed.delivery.node().0 as usize];
        if let Output::Notice {
            notice: Notice::Ask { question },
            ..
        } = failed.delivery.output
            && node
                .question
                .is_some_and(|q| q.id == question && q.post_failed)
        {
            let event = Event::Command {
                node: failed.delivery.node(),
                id: node.last_cmd + 1,
                command: Command::Answer {
                    question,
                    answer: Answer::Continue,
                },
            };
            self.apply(snapshot, world, event, commit)?;
        } else {
            let mut retry = failed.delivery;
            retry.marker_key = Some(retry.marker_key().into());
            retry.key = format!("notice-retry:{}", snapshot.revision.get() + 1);
            retry.epoch = snapshot.epoch.get();
            commit.rows.push(self.allowance_row(retry.clone()));
            commit.effects.push(NewEffect {
                key: retry.key.clone(),
                kind: "GitHub".into(),
                payload: json!(retry),
            });
            commit.rows.push(RowChange::Delete {
                table: "notice_failures".into(),
                id: row.id.clone(),
            });
        }
        Ok(true)
    }

    pub(crate) fn retry_question(
        &self,
        snapshot: &Snapshot,
        previous: &World,
        delivery: &mut Delivery,
        commit: &mut Commit,
    ) -> Result<()> {
        let Output::Notice {
            node,
            notice: Notice::Ask { question },
        } = delivery.output
        else {
            return Ok(());
        };
        let Some(old) = previous
            .nodes
            .get(node.0 as usize)
            .and_then(|n| n.question)
            .filter(|q| q.post_failed && q.id != question)
        else {
            return Ok(());
        };
        for row in snapshot
            .rows
            .iter()
            .filter(|r| r.table == "notice_failures")
        {
            let failed: FailedNotice = serde_json::from_value(row.value.clone())?;
            if !matches!(failed.delivery.output, Output::Notice { node: n, notice: Notice::Ask { question: q } } if n == node && q == old.id)
            {
                continue;
            }
            let marker = failed.delivery.marker_key().to_owned();
            let mut retry = failed.delivery;
            retry.key = delivery.key.clone();
            retry.marker_key = Some(marker);
            retry.epoch = delivery.epoch;
            retry.output = delivery.output;
            retry.question = delivery.question;
            if let Some(context) = retry.question_context.as_mut() {
                context["question"] = json!(question);
            }
            *delivery = retry;
            commit.rows.push(put(
                "question_context",
                question.to_string(),
                json!(delivery.question_context),
            ));
            commit.rows.push(RowChange::Delete {
                table: "notice_failures".into(),
                id: row.id.clone(),
            });
            if let Some(incident) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "incidents" && r.id == failed.incident)
            {
                let mut value = incident.value.clone();
                value["open"] = json!(false);
                commit
                    .rows
                    .push(put("incidents", incident.id.clone(), value));
            }
            break;
        }
        Ok(())
    }
}
