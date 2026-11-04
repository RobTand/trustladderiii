//! Approval delivery policy for Q27, approved by Rob on 2026-11-04.
use crate::{
    Controller, Result,
    executor::{Delivery, Output, Reconciliation},
    looks, put,
};
use serde_json::json;
use tl3_kernel::EffectKind;
use tl3_store::{Commit, Effect, EffectState, RowChange, Snapshot};

pub(crate) const AMBIGUOUS: &str = "approval_outcome_unknown";
pub(crate) const AUTHORIZATIONS: &str = "approval_retry_authorizations";
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
    /// #766: an old Intended row can have already sent a POST before re-epoching.
    pub(crate) fn approval_has_prior_dispatch(&mut self, effect: &Effect) -> Result<bool> {
        let mut revision = effect.created_revision;
        loop {
            let events = self.store.events_after(revision, 128)?;
            if events.is_empty() {
                return Ok(false);
            }
            for event in events {
                if event.payload["effect"] == effect.key && event.payload["to"] == "Sent" {
                    return Ok(true);
                }
                revision = event.receipt.revision;
                if revision >= effect.updated_revision {
                    return Ok(false);
                }
            }
        }
    }

    pub(crate) fn approval_ambiguous(&mut self, delivery: &Delivery) -> Result<bool> {
        let snapshot = self.store.load()?;
        if snapshot.rows.iter().any(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["effect"] == delivery.key
                && r.value["signature"]["class"] == AMBIGUOUS
        }) {
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
        self.maintenance_incident(&snapshot, delivery, &delivery.key, AMBIGUOUS,
            format!("Approval check outcome unknown for {}; clear to retry. Matching GitHub evidence resolves this automatically.", delivery.head_sha.as_deref().unwrap_or("unknown head")), &mut commit)?;
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
        if matches!(effect.state, EffectState::Sent | EffectState::Uncertain) {
            commit.transitions.push(tl3_store::EffectTransition {
                key: effect.key.clone(),
                from: effect.state,
                to: EffectState::Intended,
            });
            commit
                .rows
                .push(put(AUTHORIZATIONS, effect.key.clone(), json!(true)));
        }
    }

    pub(crate) fn queue_approval_news(
        &mut self,
        delivery: &crate::ingress::GitHubDelivery,
    ) -> Result<()> {
        let crate::ingress::GitHubDelivery::Webhook(webhook) = delivery else {
            return Ok(());
        };
        let Some(
            tl3_github::webhook::Event::CheckRunCompleted { context, check_run }
            | tl3_github::webhook::Event::CheckRunUpdated { context, check_run },
        ) = &webhook.event
        else {
            return Ok(());
        };
        let Some((key, head)) = check_run
            .external_id
            .as_deref()
            .and_then(|id| id.rsplit_once(':'))
        else {
            return Ok(());
        };
        let Some(effect) = self.store.effect(key)? else {
            return Ok(());
        };
        let approval: Delivery = crate::decode_stored(effect.payload)?;
        if !is_approval(&approval)
            || approval.head_sha.as_deref() != Some(head)
            || check_run.head_sha != head
            || approval.repository.installation != context.installation_id
            || !approval
                .repository
                .owner
                .eq_ignore_ascii_case(&context.repository.owner.login)
            || !approval
                .repository
                .name
                .eq_ignore_ascii_case(&context.repository.name)
            || !self
                .bootstrap
                .github_repositories()
                .contains(&approval.repository)
        {
            return Ok(());
        }
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("approval-news:{}", webhook.id),
            json!({"approval_news":key}),
            &snapshot,
        );
        commit.rows.push(put(REPAIRS, key.into(), json!(approval)));
        self.store.commit(&commit)?;
        Ok(())
    }

    /// Restart/catch-up look, not a polling loop or a persistent terminal watch set.
    pub(crate) fn recover_approvals(&mut self, reconcile: bool) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("approval-recovery:{}", snapshot.revision.get()),
            json!({"approval_recovery":true}),
            &snapshot,
        );
        for effect in self.store.effects()? {
            if !matches!(
                effect.state,
                EffectState::Confirmed | EffectState::Failed | EffectState::Cancelled
            ) {
                continue;
            }
            let mut delivery: Delivery = crate::decode_stored(effect.payload)?;
            if !is_approval(&delivery) {
                continue;
            }
            self.adapters
                .adapter(&delivery)?
                .retire_delivery(&delivery)?;
            if reconcile && effect.state == EffectState::Confirmed {
                delivery.epoch = snapshot.epoch.get();
                commit
                    .rows
                    .push(put(REPAIRS, delivery.key.clone(), json!(delivery)));
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }

    pub(crate) fn reconcile_approvals(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for row in snapshot.rows.iter().filter(|r| r.table == REPAIRS) {
            let mut delivery: Delivery = crate::decode_stored(row.value.clone())?;
            delivery.epoch = snapshot.epoch.get();
            match self.adapters.adapter(&delivery)?.reconcile(&delivery) {
                Ok(Reconciliation::Found(_)) => {}
                Ok(_) | Err(_) => continue,
            }
            let mut commit = Commit::new(
                format!("approval-reconciled:{}:{}", row.id, snapshot.revision.get()),
                json!({"approval_reconciled":row.id}),
                snapshot,
            );
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
