//! PR outcomes shared by delivery adapters and simulation.
use crate::findings::{CiEvidence, FailedTest};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Outcome {
    Merged {
        head: String,
        checks_passed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        evidence: Option<Box<MergeEvidence>>,
    },
    CiFailed {
        head: String,
        infra: bool,
        failed_tests: Vec<FailedTest>,
        evidence: CiEvidence,
    },
    Conflict {
        head: String,
    },
    PushByOther {
        head: String,
    },
    ClosedByPerson,
}

/// Preserve receipt identities when delivery changes adapters across a restart.
pub(crate) fn identity(repository: &crate::Repository, number: u64, suffix: &str) -> String {
    format!(
        "{}/{}/{}/{}/{suffix}",
        repository.owner.to_ascii_lowercase(),
        repository.name.to_ascii_lowercase(),
        repository.installation,
        number
    )
}

pub(crate) fn recorded(snapshot: &tl3_store::Snapshot, identity: &str) -> bool {
    snapshot
        .rows
        .iter()
        .any(|row| row.table == "delivery_outcomes" && row.id == identity)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequiredChecks {
    pub passed: bool,
    pub count: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Landed {
    pub commit: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub head_tree: String,
    pub base_before_merge: Option<String>,
    pub tree_matches: bool,
    pub parent_matches: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeEvidence {
    pub head: String,
    pub tl3_checks_passed: bool,
    pub review_approved: bool,
    pub required_checks: RequiredChecks,
    pub landed: Landed,
    pub observed: bool,
}
impl MergeEvidence {
    fn notices(&self) -> Vec<String> {
        let mut notices = Vec::new();
        if self.required_checks.count == 0 {
            notices.push("merged with no required GitHub checks".into());
        }
        if self.observed && !self.required_checks.passed {
            notices.push(
                "observed someone else's merge with required GitHub checks not passed".into(),
            );
        }
        if !self.landed.tree_matches {
            notices.push(format!(
                "landed tree differs from the checked head: landed {}, checked {}",
                self.landed.tree, self.landed.head_tree
            ));
        }
        if self.landed.parent_matches == Some(false) {
            notices.push(format!("landed first parent differs from the base read before merge: landed {:?}, base {:?}",
                self.landed.parents.first(), self.landed.base_before_merge));
        }
        notices
    }
}
impl crate::Controller {
    pub(crate) fn record_merge(
        &self,
        snapshot: &tl3_store::Snapshot,
        node: tl3_kernel::NodeId,
        evidence: &MergeEvidence,
        commit: &mut tl3_store::Commit,
    ) -> crate::Result<()> {
        use crate::{
            executor::{Delivery, Output, Upstream},
            looks, put,
        };
        use serde_json::json;
        let id = format!("{}:{}", node.0, evidence.head);
        commit
            .rows
            .push(put("merge_records", id.clone(), json!(evidence)));
        let request = crate::request(snapshot, node)?;
        let world = crate::snapshot::decode(snapshot)?;
        let n = world
            .nodes
            .get(node.0 as usize)
            .ok_or(crate::Error::Invalid("merge record node missing"))?;
        for (index, summary) in evidence.notices().into_iter().enumerate() {
            let id = format!("merge-audit:{id}:{index}");
            let signature = looks::IncidentSignature {
                upstream: Upstream::Delivery,
                class: "merge_audit".into(),
            };
            let mut post = Delivery {
                key: format!("incident:{id}"),
                epoch: snapshot.epoch.get(),
                output: Output::Notice {
                    node,
                    notice: tl3_kernel::Notice::Incident { needs_rob: false },
                },
                generation: n.generation,
                head_sha: Some(evidence.head.clone()),
                repository: request.repo.clone(),
                issue: request.issue,
                pull_request: None,
                proposed: None,
                paths: tl3_contracts::footprint::Footprint::new(vec![])
                    .map_err(crate::Error::Invalid)?,
                attempt: n.attempt,
                round: n.round,
                replanned: n.replanned,
                checks: None,
                job_generation: None,
                question: None,
                marker_key: None,
                note: Some(looks::incident_note(&request.repo, request.issue, &summary)),
                scope_removals: None,
                question_context: Some(
                    json!({"incident_id":id,"signature":signature,"issue_title":request.title}),
                ),
                steering: None,
                request_text: None,
                evidence: None,
            };
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
                json!(looks::Incident {
                    id,
                    node: Some(node),
                    input_failure: None,
                    open: false,
                    needs_rob: false,
                    blocking: false,
                    summary,
                    signature,
                    effect: None,
                    post_key: Some(post.key.clone()),
                    cause: None,
                }),
            ));
            commit.rows.push(self.allowance_row(post.clone()));
            commit.effects.push(post.into_effect());
        }
        Ok(())
    }
}
