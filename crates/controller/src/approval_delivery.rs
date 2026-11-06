//! Approval delivery policy for Q27, approved by Rob on 2026-11-04.
use crate::{
    Controller, Result,
    executor::{Delivery, Output, Reconciliation},
    looks, put,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tl3_kernel::EffectKind;
use tl3_store::{Commit, Effect, EffectState, RowChange, Snapshot};

pub(crate) const AMBIGUOUS: &str = "approval_outcome_unknown";
pub(crate) const AUTHORIZATIONS: &str = "approval_retry_authorizations";
pub(crate) const LINEAGE_SAFE: &str = "approval_lineage_admitted";
pub(crate) const UNRESOLVED: &str = "approval_dispatch_recovery";
pub(crate) const REPAIRS: &str = "approval_reconciliations";

pub(crate) fn is_approval(delivery: &Delivery) -> bool {
    matches!(
        delivery.output,
        Output::Effect {
            kind: EffectKind::PostApproval,
            ..
        }
    )
}

impl Controller {
    /// #766: legacy retries carry dispatch ambiguity across replacement sequences.
    pub(crate) fn approval_prior_dispatch(
        &mut self,
        effect: &Effect,
        snapshot: &Snapshot,
    ) -> Result<(bool, bool)> {
        let delivery: Delivery = crate::decode_stored(effect.payload.clone())?;
        let identity = json!({
            "repository":delivery.repository, "issue":delivery.issue,
            "generation":delivery.generation, "head_sha":delivery.head_sha,
            "output":{"Effect":{"node":delivery.node(),"kind":"PostApproval"}}
        });
        // Dispatch records are indexed by operation and key. The bounded legacy
        // migration completes before new dispatch is admitted.
        let predecessor = !snapshot
            .rows
            .iter()
            .any(|r| r.table == LINEAGE_SAFE && r.id == effect.key)
            && self
                .store
                .dispatched_effect_matching(&identity, &effect.key)?;
        let current = self
            .store
            .dispatched_effect_matching(&json!({"key":effect.key}), "")?;
        Ok((current || predecessor, predecessor))
    }

    pub(crate) fn approval_ambiguous(&mut self, delivery: &Delivery) -> Result<bool> {
        let snapshot = self.store.load()?;
        let incident_open = snapshot.rows.iter().any(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["effect"] == delivery.key
                && r.value["signature"]["class"] == AMBIGUOUS
        });
        if incident_open
            && snapshot
                .rows
                .iter()
                .any(|r| r.table == UNRESOLVED && r.id == delivery.key)
        {
            return Ok(false);
        }
        let mut commit = Commit::new(
            format!(
                "approval-ambiguous:{}:{}",
                delivery.key,
                snapshot.revision.get()
            ),
            json!({"approval_ambiguous":delivery.key}),
            &snapshot,
        );
        commit
            .rows
            .push(put(UNRESOLVED, delivery.key.clone(), json!(delivery)));
        if !incident_open {
            self.maintenance_incident(&snapshot, delivery, &delivery.key, AMBIGUOUS,
            format!("Approval check outcome unknown for {}; clear to retry. Matching GitHub evidence resolves this automatically.", delivery.head_sha.as_deref().unwrap_or("unknown head")), &mut commit)?;
        }
        self.store.commit(&commit)?;
        Ok(true)
    }

    /// Q27 decision point: explicit human clear authorizes one GET-then-POST retry.
    /// It cannot retract outstanding POSTs; all resulting copies are reconciled.
    pub(crate) fn authorize_approval_retry(
        &self,
        snapshot: &Snapshot,
        incident: &looks::Incident,
        commit: &mut Commit,
    ) {
        if incident.signature.class != AMBIGUOUS || !incident.open {
            return;
        }
        let Some(effect) = snapshot
            .pending_effects
            .iter()
            .find(|effect| Some(&effect.key) == incident.effect.as_ref())
        else {
            return;
        };
        if tl3_delivery::can_authorize_approval_retry(crate::executor::delivery_state(effect.state))
        {
            commit.transitions.push(tl3_store::EffectTransition {
                key: effect.key.clone(),
                from: effect.state,
                to: EffectState::Intended,
            });
            commit
                .rows
                .push(put(AUTHORIZATIONS, effect.key.clone(), json!(true)));
            commit
                .rows
                .push(put(LINEAGE_SAFE, effect.key.clone(), json!(true)));
        }
    }

    pub(crate) fn queue_approval_news(
        &mut self,
        delivery: &crate::ingress::GitHubDelivery,
    ) -> Result<()> {
        let updates = match delivery {
            crate::ingress::GitHubDelivery::Webhook(webhook) => {
                let Some(
                    tl3_github::webhook::Event::CheckRunCompleted { context, check_run }
                    | tl3_github::webhook::Event::CheckRunUpdated { context, check_run },
                ) = &webhook.event
                else {
                    return Ok(());
                };
                vec![(
                    crate::Repository {
                        owner: context.repository.owner.login.clone(),
                        name: context.repository.name.clone(),
                        installation: context.installation_id,
                    },
                    webhook.id.clone(),
                    check_run,
                )]
            }
            crate::ingress::GitHubDelivery::CatchUp {
                repository,
                changes,
            } => {
                let mut updates = Vec::new();
                for check in &changes.check_runs {
                    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(check)?));
                    updates.push((
                        repository.clone(),
                        format!(
                            "catch-up:{}/{}/{}:{}:{digest}",
                            repository.owner, repository.name, repository.installation, check.id
                        ),
                        check,
                    ));
                }
                updates
            }
        };
        for (repository, news, check_run) in updates {
            let Some((key, head)) = check_run
                .external_id
                .as_deref()
                .and_then(|id| id.rsplit_once(':'))
            else {
                continue;
            };
            let Some(effect) = self.store.effect(key)? else {
                continue;
            };
            let approval: Delivery = crate::decode_stored(effect.payload)?;
            if !is_approval(&approval)
                || approval.head_sha.as_deref() != Some(head)
                || check_run.head_sha != head
                || approval.repository.installation != repository.installation
                || !approval
                    .repository
                    .owner
                    .eq_ignore_ascii_case(&repository.owner)
                || !approval
                    .repository
                    .name
                    .eq_ignore_ascii_case(&repository.name)
                || !self
                    .bootstrap
                    .github_repositories()
                    .contains(&approval.repository)
            {
                continue;
            }
            let snapshot = self.store.load()?;
            let mut commit = Commit::new(
                format!("approval-news:{news}"),
                json!({"approval_news":key}),
                &snapshot,
            );
            commit.rows.push(put(REPAIRS, key.into(), json!(approval)));
            self.store.commit(&commit)?;
        }
        Ok(())
    }

    pub(crate) fn reconcile_approvals(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for row in snapshot.rows.iter().filter(|r| r.table == REPAIRS) {
            let mut delivery: Delivery = crate::decode_stored(row.value.clone())?;
            delivery.epoch = snapshot.epoch.get();
            let repair_key = format!("approval-repair:{}", row.id);
            if looks::outage_waiting(snapshot, &repair_key) {
                continue;
            }
            let result = self
                .adapters
                .adapter(&delivery)
                .and_then(|adapter| adapter.repair_approval(&delivery));
            if !matches!(result, Ok((Reconciliation::Found(_), _))) {
                let error = match result {
                    Err(error) => error,
                    _ => crate::Error::Invalid("approval repair has no matching evidence yet"),
                };
                let mut commit = Commit::new(
                    format!(
                        "approval-repair-failed:{}:{}",
                        row.id,
                        snapshot.revision.get()
                    ),
                    json!({"approval_repair_failed":row.id}),
                    snapshot,
                );
                self.maintenance_failure(
                    snapshot,
                    &delivery,
                    &repair_key,
                    "approval_repair",
                    error,
                    &mut commit,
                )?;
                self.store.commit(&commit)?;
                return Ok(true);
            }
            let mut commit = Commit::new(
                format!("approval-reconciled:{}:{}", row.id, snapshot.revision.get()),
                json!({"approval_reconciled":row.id}),
                snapshot,
            );
            self.finish_maintenance(snapshot, &repair_key, &mut commit)?;
            // Count only identity-checked copies from the adapter's successful repair.
            // Sent intent overestimates unknown legacy/preflight attempts conservatively.
            if let Ok((_, Some(copies))) = result
                && tl3_delivery::accounts_for_dispatches(
                    copies as u64,
                    self.store.approval_dispatch_count(&row.id)? as u64,
                )
            {
                commit.rows.push(RowChange::Delete {
                    table: UNRESOLVED.into(),
                    id: row.id.clone(),
                });
            }
            commit.rows.push(RowChange::Delete {
                table: REPAIRS.into(),
                id: row.id.clone(),
            });
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }
}
