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
    pub commit: Option<String>,
    pub tree: Option<String>,
    pub checked_base: Option<String>,
    pub merge_base: Option<String>,
    pub pr_base_sha: Option<String>,
    pub base_moved: Option<bool>,
    pub checked_parent_matches: Option<bool>,
    pub parents: Option<Vec<String>>,
    pub head_tree: Option<String>,
    pub base_before_merge: Option<String>,
    pub tree_matches: Option<bool>,
    pub parent_matches: Option<bool>,
    pub parent_comparison_reason: Option<String>,
    pub method: Option<tl3_github::MergeMethod>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeEvidence {
    pub head: String,
    pub tl3_checks_passed: Option<bool>,
    pub review_approved: Option<bool>,
    pub required_checks: Option<RequiredChecks>,
    pub landed: Landed,
    pub observed: bool,
    pub evidence_errors: Vec<EvidenceError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceError {
    pub read: String,
    pub class: String,
}
impl MergeEvidence {
    pub(crate) fn empty(head: String) -> Self {
        Self {
            head,
            tl3_checks_passed: None,
            review_approved: None,
            required_checks: None,
            observed: false,
            evidence_errors: vec![],
            landed: Landed {
                commit: None,
                tree: None,
                checked_base: None,
                merge_base: None,
                pr_base_sha: None,
                base_moved: None,
                checked_parent_matches: None,
                parents: None,
                head_tree: None,
                base_before_merge: None,
                tree_matches: None,
                parent_matches: None,
                parent_comparison_reason: None,
                method: None,
            },
        }
    }
    pub(crate) fn error(&mut self, read: &str, class: impl Into<String>) {
        self.evidence_errors.push(EvidenceError {
            read: read.into(),
            class: class.into(),
        });
    }
    pub(crate) fn compare(&mut self) {
        let landed = &mut self.landed;
        landed.base_moved = landed
            .checked_base
            .as_ref()
            .zip(landed.merge_base.as_ref())
            .map(|(checked, current)| checked != current);
        landed.tree_matches = landed
            .tree
            .as_ref()
            .zip(landed.head_tree.as_ref())
            .map(|(actual, checked)| actual == checked);
        if landed.method == Some(tl3_github::MergeMethod::Rebase) {
            landed.parent_matches = None;
            landed.checked_parent_matches = None;
            landed.parent_comparison_reason = Some("rebase merge method".into());
        } else {
            landed.parent_matches = landed
                .parents
                .as_ref()
                .zip(landed.base_before_merge.as_ref())
                .map(|(parents, base)| parents.first() == Some(base));
            landed.checked_parent_matches = landed
                .parents
                .as_ref()
                .zip(landed.checked_base.as_ref())
                .map(|(parents, base)| parents.first() == Some(base));
        }
    }
    fn notices(&self) -> Vec<String> {
        let mut notices = Vec::new();
        if self
            .required_checks
            .as_ref()
            .is_some_and(|checks| checks.count == 0)
        {
            notices.push("merged with no required GitHub checks".into());
        }
        if self.observed
            && self
                .required_checks
                .as_ref()
                .is_some_and(|checks| !checks.passed)
        {
            notices.push(
                "observed someone else's merge with required GitHub checks not passed".into(),
            );
        }
        if self.landed.tree_matches == Some(false) {
            notices.push(format!(
                "landed tree differs from the checked head: landed {}, checked {}",
                self.landed.tree.as_deref().unwrap_or_default(),
                self.landed.head_tree.as_deref().unwrap_or_default()
            ));
        }
        if self.landed.parent_matches == Some(false) {
            notices.push(format!("landed first parent differs from the base read before merge: landed {:?}, base {:?}",
                self.landed.parents.as_ref().and_then(|parents| parents.first()), self.landed.base_before_merge));
        }
        if !self.evidence_errors.is_empty() {
            let reads: std::collections::BTreeSet<_> = self
                .evidence_errors
                .iter()
                .map(|error| error.read.as_str())
                .collect();
            notices.push(format!(
                "merge evidence incomplete: {}",
                reads.into_iter().collect::<Vec<_>>().join(", ")
            ));
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bootstrap, Controller, LabelledIssue, Repository, adapters, executor::Observation};
    use serde_json::json;
    use tl3_kernel::{Event, Fact, NodeId};
    use tl3_store::{Commit, RowChange, Store};

    #[test]
    fn m4_r2_merge_record_metadata_failure_cannot_suppress_fact() {
        let mut database = crate::store_support::Database::start();
        let settings = database.create("m4_r2_record_failure");
        let root = tempfile::tempdir().unwrap();
        let mut controller = Controller::open(
            Store::open(&settings).unwrap(),
            adapters::simulation(root.path()).unwrap(),
            Bootstrap {
                repos: vec![Repository {owner:"owner".into(), name:"sandbox".into(), installation:3}],
                rob:42, slots:1, pool:1, fail_fast:false, operations:None,
            },
        ).unwrap();
        controller.intake(LabelledIssue {
            delivery:"m4-r2-record".into(), owner:"owner".into(), repo:"sandbox".into(), installation:3,
            number:12, title:"merge record".into(), actor:42, label:"tl3".into(), labels:vec![],
        }).unwrap();
        let original = controller.snapshot().unwrap();
        for corrupt in [false, true] {
            let mut snapshot = original.clone();
            if corrupt {
                snapshot.rows.iter_mut().find(|r|r.table == "requests").unwrap().value = json!("invalid request");
            } else { snapshot.rows.retain(|r|r.table != "requests"); }
            let mut world = crate::snapshot::decode(&snapshot).unwrap();
            let mut commit = Commit::new(format!("m4-r2-record-{corrupt}"), json!({}), &snapshot);
            let observation = Observation::Outcome {
                node:NodeId(0), receipt:Some(format!("m4-r2-merged-{corrupt}")),
                outcome:Outcome::Merged {head:"a".repeat(40), checks_passed:true, evidence:Some(Box::new(MergeEvidence::empty("a".repeat(40))))},
            };
            let result = controller.translate(&snapshot, &mut world, observation, &mut commit, &mut crate::feedback::Evidence::default());
            assert!(matches!(result, Ok(Some(Event::Fact {node:NodeId(0), fact:Fact::PrMerged {checks_passed:true,..}}))), "record metadata failure cannot suppress the known fact: {result:?}");
            let records: Vec<_> = commit.rows.iter().filter_map(|row| match row {
                RowChange::Put {table,value,..} if table == "merge_records" => Some(value),
                _ => None,
            }).collect();
            assert_eq!(records.len(), 1, "rollback partial audit rows before recording degraded evidence");
            assert!(records[0]["evidence_errors"].as_array().unwrap().iter().any(|error|error["read"] == "record_merge"));
        }
    }
}
