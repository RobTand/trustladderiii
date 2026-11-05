//! Q53 / #828: durable availability evidence and bounded quota probes.
use super::*;
use crate::quota_wake::clears_hold;

const HOUR_MS: u64 = 3_600_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaModel {
    pub model: String,
    pub family: String,
}
impl QuotaModel {
    fn key(&self) -> String {
        serde_json::to_string(self).expect("quota identity serializes")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProbeOwner {
    key: String,
    after_attempt: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Probe {
    identity: QuotaModel,
    next_ms: u64,
    owner: Option<ProbeOwner>,
}

pub(super) fn success_after(snapshot: &Snapshot, model: &str, family: &str, quota_ms: i64) -> bool {
    snapshot
        .rows
        .iter()
        .filter(|row| row.table == "model_successes")
        .any(|row| {
            row.value["finished_ms"].as_i64().is_some_and(|success_ms| {
                clears_hold(
                    row.value["identity"]["model"].as_str() == Some(model),
                    row.value["identity"]["family"].as_str() == Some(family),
                    success_ms,
                    quota_ms,
                )
            })
        })
}

impl Controller {
    /// Q53 / #828: six hours trusts ordinary resets but bounds multi-day stale hints.
    pub fn set_model_quota_policy(
        &mut self,
        models: &tl3_config::Models,
        horizon: Duration,
    ) -> Result<()> {
        if horizon.is_zero() {
            return Err(crate::Error::Invalid(
                "model quota horizon must be positive",
            ));
        }
        let mut families = std::collections::BTreeMap::new();
        for model in [
            &models.ingest,
            &models.audit,
            &models.planner,
            &models.maker,
            &models.reviewer,
            &models.rebaser,
            &models.comment_classifier,
            &models.writer,
            &models.postmortem,
        ]
        .into_iter()
        .flatten()
        {
            let family = model.family.to_ascii_lowercase();
            if families
                .insert(&model.name, family.clone())
                .is_some_and(|old| old != family)
            {
                return Err(crate::Error::Invalid(
                    "one model name has conflicting families",
                ));
            }
        }
        self.quota_models = Some(models.clone());
        self.quota_horizon_ms = horizon.as_millis().min(u128::from(u64::MAX)) as u64;
        Ok(())
    }

    pub(super) fn quota_family(&self, name: &str) -> Option<String> {
        let m = self.quota_models.as_ref()?;
        [
            &m.ingest,
            &m.audit,
            &m.planner,
            &m.maker,
            &m.reviewer,
            &m.rebaser,
            &m.comment_classifier,
            &m.writer,
            &m.postmortem,
        ]
        .into_iter()
        .flatten()
        .find(|model| model.name == name)
        .map(|model| model.family.to_ascii_lowercase())
    }

    pub(super) fn attempt_family(
        &self,
        snapshot: &Snapshot,
        key: &str,
        attempt: i64,
        model: &str,
    ) -> Option<String> {
        let id = serde_json::to_string(&(key, attempt)).expect("attempt identity serializes");
        snapshot
            .rows
            .iter()
            .find(|row| {
                row.table == "model_attempts" && row.id == id && row.value["model"] == model
            })
            .and_then(|row| row.value["family"].as_str().map(str::to_owned))
            .or_else(|| self.quota_family(model))
    }

    /// Imported completion facts are authoritative even for a reply whose node moved on.
    /// The watermark and all affected deadlines commit with the observation's inbox row.
    pub(crate) fn model_success_evidence(
        &mut self,
        snapshot: &Snapshot,
        observation: &Observation,
        commit: &mut Commit,
    ) -> Result<()> {
        let ending = match observation {
            Observation::WorkerReply {
                node,
                role,
                reply,
                model: Some(model),
                ..
            } if crate::adapters::workers::protocol::map_reply(role, reply).is_ok() => {
                self.store
                    .latest_worker_success(&node.0.to_string(), model)?
            }
            Observation::Classified {
                key,
                reply: Some(_),
                ..
            }
            | Observation::Postmortem {
                key,
                reply: Some(_),
                ..
            } => self.store.latest_worker_ending(key)?,
            Observation::WorkerFailed {
                node, generation, ..
            } => {
                let key = snapshot
                    .rows
                    .iter()
                    .find(|row| {
                        row.table == "allowances"
                            && row.value["delivery"]["output"]["Effect"]["node"] == node.0
                            && row.value["delivery"]["generation"] == *generation
                    })
                    .map(|row| row.id.clone());
                if let Some(key) = key {
                    let ending = self.store.latest_worker_ending(&key)?;
                    self.release_model_probe(
                        snapshot,
                        &key,
                        ending.as_ref().map(|e| e.attempt),
                        commit,
                    )?;
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        let Some(ending) = ending else {
            return Ok(());
        };
        self.release_model_probe(snapshot, &ending.key, Some(ending.attempt), commit)?;
        if ending.facts.outcome != "succeeded" {
            return Ok(());
        }
        let Some(finished_ms) = ending.facts.finished_ms else {
            return Ok(());
        };
        let Some(family) =
            self.attempt_family(snapshot, &ending.key, ending.attempt, &ending.facts.model)
        else {
            return Ok(());
        };
        let identity = QuotaModel {
            model: ending.facts.model,
            family,
        };
        let id = identity.key();
        if snapshot.rows.iter().any(|row| {
            row.table == "model_successes"
                && row.id == id
                && row.value["finished_ms"]
                    .as_i64()
                    .is_some_and(|old| old >= finished_ms)
        }) {
            return Ok(());
        }
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|row| row.table == "model_quota_incidents" && row.id == id)
            && let Some(id) = row.value["incident"].as_str()
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|row| row.table == "incidents" && row.id == id)
        {
            let mut incident: Incident = crate::decode_stored(row.value.clone())?;
            if incident.open && !incident.needs_rob {
                incident.open = false;
                commit
                    .rows
                    .push(put("incidents", id.into(), json!(incident)));
            }
        }
        commit.rows.push(put(
            "model_successes",
            id,
            json!({"identity": identity, "finished_ms": finished_ms}),
        ));
        let mut cleared_model = false;
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "model_quotas")
        {
            if row.value["finished_ms"].as_i64().is_some_and(|quota_ms| {
                clears_hold(
                    row.id == identity.model,
                    row.value["family"].as_str() == Some(&identity.family),
                    finished_ms,
                    quota_ms,
                )
            }) {
                cleared_model = true;
                commit.rows.push(RowChange::Delete {
                    table: row.table.clone(),
                    id: row.id.clone(),
                });
            }
        }
        for row in snapshot.rows.iter().filter(|row| row.table == "allowances") {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            let ladder_available = cleared_model
                && self
                    .adapters
                    .adapter(&state.delivery)?
                    .quota_candidates(&state.delivery)?
                    .contains(&identity);
            if state.quota_ms.is_some()
                && (ladder_available
                    || state.last_quota_finished_ms.is_some_and(|quota_ms| {
                        clears_hold(
                            state.last_quota_model.as_deref() == Some(&identity.model),
                            state.last_quota_family.as_deref() == Some(&identity.family),
                            finished_ms,
                            quota_ms,
                        )
                    }))
            {
                state.due_ms = Some(self.now_ms());
                state.quota_retry_ms = state.due_ms;
                commit
                    .rows
                    .push(put("allowances", row.id.clone(), json!(state)));
            }
        }
        Ok(())
    }

    pub(super) fn release_model_probe(
        &self,
        snapshot: &Snapshot,
        key: &str,
        attempt: Option<i64>,
        commit: &mut Commit,
    ) -> Result<()> {
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "model_quota_probes")
        {
            let mut probe: Probe = crate::decode_stored(row.value.clone())?;
            if probe.owner.as_ref().is_some_and(|owner| {
                owner.key == key && attempt.is_some_and(|attempt| attempt > owner.after_attempt)
            }) {
                probe.owner = None;
                commit
                    .rows
                    .push(put("model_quota_probes", row.id.clone(), json!(probe)));
                for held in snapshot
                    .rows
                    .iter()
                    .filter(|row| row.table == "allowances" && row.id != key)
                {
                    let mut state: AllowanceState = crate::decode_stored(held.value.clone())?;
                    if state.quota_ms.is_some()
                        && state.quota_probe_model.as_ref() == Some(&probe.identity)
                    {
                        state.due_ms = Some(state.due_ms.unwrap_or(u64::MAX).min(probe.next_ms));
                        commit
                            .rows
                            .push(put("allowances", held.id.clone(), json!(state)));
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn overdue_model_quota(
        &self,
        snapshot: &Snapshot,
        state: &AllowanceState,
        commit: &mut Commit,
    ) -> Result<()> {
        let Some(identity) = &state.quota_probe_model else {
            return Ok(());
        };
        let scope = format!("model-quota:{}", identity.key());
        let request = crate::request(snapshot, state.delivery.node())?;
        self.input_failed(
            snapshot,
            &scope,
            Some(crate::input_errors::IssueTarget {
                repository: request.repo,
                issue: request.issue,
            }),
            &request.title,
            Some(state.delivery.node()),
            Upstream::Workers,
            crate::Error::Permanent {
                class: "model_quota_hint_expired",
                source: Box::new(crate::Error::Invalid("quota probe failed after reset")),
            },
            commit,
        )?;
        if let Some(id) = commit.rows.iter().rev().find_map(|row| match row {
            RowChange::Put { table, id, value }
                if table == "incidents"
                    && value["signature"]["class"] == "model_quota_hint_expired" =>
            {
                Some(id.clone())
            }
            _ => None,
        }) {
            commit.rows.push(put(
                "model_quota_incidents",
                identity.key(),
                json!({"incident": id}),
            ));
        }
        Ok(())
    }

    pub(super) fn quota_retry(
        &mut self,
        state: &mut AllowanceState,
        resume: u64,
        now: u64,
    ) -> Result<Option<u64>> {
        // A bounded probe remains bounded as the hint approaches. Its original
        // decision is durable; restarting or reaching H cannot drop the lease.
        if state.quota_probe_at_ms.is_none() && resume.saturating_sub(now) <= self.quota_horizon_ms
        {
            return Ok(Some(resume));
        }
        let Some(identity) = self
            .adapters
            .adapter(&state.delivery)?
            .quota_candidates(&state.delivery)?
            .into_iter()
            .next()
        else {
            return Ok(Some(resume));
        };
        let snapshot = self.store.load()?;
        // Only ordered availability evidence ends a bounded probe regime early.
        // Merely expiring the original hint cannot authorize a second probe.
        if state
            .last_quota_finished_ms
            .zip(state.last_quota_model.as_deref())
            .zip(state.last_quota_family.as_deref())
            .is_some_and(|((quota_ms, model), family)| {
                success_after(&snapshot, model, family, quota_ms)
            })
        {
            return Ok(Some(now));
        }
        state.quota_probe_model = Some(identity.clone());
        let id = identity.key();
        let mut probe = snapshot
            .rows
            .iter()
            .find(|row| row.table == "model_quota_probes" && row.id == id)
            .map(|row| crate::decode_stored::<Probe>(row.value.clone()))
            .transpose()?
            .unwrap_or(Probe {
                identity,
                next_ms: now.saturating_add(HOUR_MS),
                owner: None,
            });
        state.quota_probe_at_ms = Some(probe.next_ms);
        let retry = if let Some(owner) = &probe.owner {
            if owner.key == state.delivery.key {
                // Recovery reuses the dispatch reservation. The adapter compares
                // the held attempt number before creating a replacement.
                Some(now)
            } else {
                None
            }
        } else if now >= probe.next_ms {
            let after_attempt = self
                .store
                .worker_progress(None)?
                .into_iter()
                .find(|p| p.key == state.delivery.key)
                .map_or(-1, |p| p.attempt);
            probe.owner = Some(ProbeOwner {
                key: state.delivery.key.clone(),
                after_attempt,
            });
            probe.next_ms = now.saturating_add(HOUR_MS);
            Some(now)
        } else {
            Some(probe.next_ms)
        };
        let mut commit = Commit::new(
            format!(
                "quota-probe:{}:{}",
                state.delivery.key,
                snapshot.revision.get()
            ),
            json!({"quota_probe": state.delivery.key}),
            &snapshot,
        );
        // The lease and held deadline precede the adapter's filesystem dispatch.
        state.quota_retry_ms = retry.or(state.quota_ms);
        state.due_ms = retry;
        commit
            .rows
            .push(put("model_quota_probes", id, json!(probe)));
        commit
            .rows
            .push(put("allowances", state.delivery.key.clone(), json!(state)));
        self.store.commit(&commit)?;
        Ok(retry)
    }
}
