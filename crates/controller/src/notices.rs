use crate::executor::{CommentKind, Delivery, Output};
use crate::looks::Incident;
use crate::{Controller, Result, put, request, snapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tl3_kernel::Notice;
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Serialize, Deserialize)]
pub(crate) struct FailedNotice {
    pub delivery: Delivery,
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
        let request = request(snapshot, node)?;
        let id = crate::looks::incident_id(snapshot, "post");
        let signature = crate::looks::IncidentSignature {
            upstream: crate::executor::Upstream::GitHub,
            class: class.into(),
        };
        let marker = delivery.marker_key();
        let incidents = snapshot
            .rows
            .iter()
            .find(|r| r.table == "notice_incidents" && r.id == marker)
            .and_then(|r| r.value.as_u64())
            .unwrap_or(0)
            .saturating_add(1);
        let needs_rob =
            crate::looks::incident_needs_rob(crate::looks::IncidentCause::NoticeFailure {
                incidents,
            });
        commit
            .rows
            .push(put("notice_incidents", marker.into(), json!(incidents)));
        let summary = format!(
            "{} The {} could not be posted. Failure class {}.",
            crate::looks::incident_summary(&request.title, &signature, needs_rob, None),
            kind(delivery),
            class.replace('_', " ")
        );
        let mut post = delivery.clone();
        post.key = format!("incident:{id}");
        post.marker_key = None;
        post.epoch = snapshot.epoch.get();
        post.output = Output::Notice {
            node,
            notice: Notice::Incident { needs_rob },
        };
        post.steering = None;
        post.note = Some(crate::looks::incident_note(
            &request.repo,
            request.issue,
            &summary,
        ));
        post.question_context = Some(json!({"issue_title":request.title,
            "request":format!("{} (https://github.com/{}/{}/issues/{})", request.title, request.repo.owner, request.repo.name, request.issue),
            "signature":signature, "incident_id":id}));
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
                node: Some(node),
                input_failure: None,
                open: true,
                needs_rob,
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
            }),
        ));
        self.link_allowance_incident(snapshot, &delivery.key, &id, commit)?;
        commit.rows.push(self.allowance_row(post.clone()));
        commit.effects.push(post.into_effect());
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
        commit.rows.push(RowChange::Delete {
            table: "notice_failures".into(),
            id: row.id.clone(),
        });
        let mut retry = failed.delivery;
        if let Output::Notice {
            node,
            notice: Notice::Ask { question },
        } = retry.output
        {
            let world = snapshot::decode(snapshot)?;
            if !world.nodes[node.0 as usize]
                .question
                .is_some_and(|q| q.id == question)
            {
                return Ok(true);
            }
        }
        retry.marker_key = Some(retry.marker_key().into());
        retry.key = format!("notice-retry:{}", snapshot.revision.get() + 1);
        retry.epoch = snapshot.epoch.get();
        commit.rows.push(self.allowance_row(retry.clone()));
        commit.effects.push(retry.into_effect());
        Ok(true)
    }
}
