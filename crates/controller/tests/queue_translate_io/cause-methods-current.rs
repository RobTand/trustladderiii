use crate::queue_cause::{CauseEvidence, QueueContext, GateEvidence, Hold, held, key};
use crate::queue_cause_decide::{self as decision, Attribution};
use chrono::{DateTime, Utc};
impl Shell<'_> {
    pub(crate) fn queue_gate(
        &self,
        snapshot: &Snapshot,
        fact: &QueueFact,
        gate: &GateEvidence,
        commit: &mut Commit,
    ) -> Result<()> {
        let identity = fact.identity();
        if snapshot
            .rows
            .iter()
            .any(|row| row.table == "queue_gates" && row.id == identity)
        {
            return Ok(());
        }
        commit.rows.push(put("queue_gates", identity, json!(fact)));
        if !gate.successful {
            return Ok(());
        }
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_holds")
        {
            let mut hold: Hold = decode_stored(row.value.clone())?;
            self.queue_hold_witness(snapshot, &fact.repository, &mut hold, gate, commit);
        }
        Ok(())
    }

    fn queue_hold_witness(
        &self,
        snapshot: &Snapshot,
        repository: &Repository,
        hold: &mut Hold,
        gate: &GateEvidence,
        commit: &mut Commit,
    ) -> bool {
            if !gate.healthy_for(&hold.reporters)
                || hold.repository != *repository
                || hold.context.branch != gate.context.branch
                || hold.context.base == gate.context.base
                || decision::hold(true, hold.witness.is_some(), false)
                    != decision::HoldAction::Requeue
                || !hold.failed_at.is_some_and(|failed| gate.at > failed)
            {
                return false;
            }
            hold.witness = Some(gate.clone());
            hold.witnessed_ms = Some(self.now_ms());
            commit
                .rows
                .push(put("queue_holds", key(hold.node, &hold.head), json!(hold)));
            // Wake an existing allowance, without creating a second timer.
            for allowance in snapshot.rows.iter().filter(|row| {
                row.table == "allowances"
                    && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
                    && row.value["delivery"]["output"]["Effect"]["node"] == hold.node.0
                    && row.value["delivery"]["head_sha"] == hold.head
            }) {
                let mut value = allowance.value.clone();
                value["due_ms"] = json!(self.now_ms());
                commit
                    .rows
                    .push(put("allowances", allowance.id.clone(), value));
            }
        true
    }

    /// Return true when the failure was journaled as a hold, without a kernel
    /// event. This is what prevents work charges and fail_fast on first refusal.
    pub(crate) fn queue_failure_cause(
        &self,
        snapshot: &Snapshot,
        fact: &mut QueueFact,
        node: NodeId,
        q58: bool,
        commit: &mut Commit,
    ) -> Result<bool> {
        let incoming_identity = fact.identity();
        let Evidence::CiFailed {
            head,
            head_check,
            completed_at,
            details,
            infra,
            ..
        } = &mut fact.evidence
        else {
            return Ok(false);
        };
        let prior = held(snapshot, node, head)?;
        let q58_used = snapshot.rows.iter().any(|row| {
            row.table == "queue_facts"
                && row.value["repository"] == json!(fact.repository)
                && row.value["number"] == fact.number
                && row.value["evidence"]["CiFailed"]["head"] == *head
                && row.value["evidence"]["CiFailed"]["infra"] == true
                && row.value["evidence"]["CiFailed"]["details"]["decision"] == "outside_change"
        });
        let retry = prior.as_ref().and_then(|hold| hold.retry.as_ref());
        let retried = (q58_used && details.failed_tests.is_empty())
            || decision::retry_failure(retry.is_some(), retry.is_some_and(|retry| retry.before.contains(&incoming_identity)));
        let context = details
            .context
            .clone()
            .map(|context| *context)
            .unwrap_or(QueueContext {
                base: String::new(),
                branch: String::new(),
                batch: None,
                members: vec![],
            });
        let matched = details.fingerprint.as_ref().and_then(|fingerprint| {
            snapshot.rows.iter().find(|row| {
                if row.table != "queue_gates" || row.value["repository"] != json!(fact.repository) {
                    return false;
                }
                let gate = &row.value["evidence"]["Gate"];
                gate["fingerprint"] == *fingerprint
                    && gate["successful"] == false
                    && !context.base.is_empty()
                    && gate["context"]["base"] == context.base
                    && gate["context"]["branch"] == context.branch
                    && gate["context"]["batch"]
                        .as_str()
                        .is_some_and(|batch| Some(batch) != context.batch.as_deref())
                    && gate["context"]["members"]
                        .as_array()
                        .is_some_and(|members| {
                            !members.is_empty() && !members.contains(&json!(fact.number))
                        })
            })
        });
        let healthy_base = !context.base.is_empty()
            && context.members == [fact.number]
            && snapshot.rows.iter().any(|row| {
                row.table == "queue_gates"
                    && row.value["repository"] == json!(fact.repository)
                    && row.value["evidence"]["Gate"]["successful"] == true
                    && row.value["evidence"]["Gate"]["context"]["base"] == context.base
                    && row.value["evidence"]["Gate"]["context"]["branch"] == context.branch
            });
        let kind = decision::attribute(
            *head_check || !details.failed_tests.is_empty() || healthy_base,
            matched.is_some(),
            retried,
        );
        let fp = details.fingerprint.clone().unwrap_or_else(|| {
            format!(
                "queue:{}:{}",
                head,
                completed_at.as_deref().unwrap_or("unknown")
            )
        });
        details.cause = Some(Box::new(match kind {
            Attribution::Change => CauseEvidence::Change { fingerprint: fp },
            Attribution::Environment => CauseEvidence::Environment {
                fingerprint: fp,
                base: context.base.clone(),
                witness: matched.expect("matched evidence").id.clone(),
            },
            Attribution::Unattributed => CauseEvidence::Unattributed,
        }));
        // Preserve Q58's complete, parsed outside-change decision and its one
        // retry. A prior opaque retest, however, can never enter Q58 again.
        if retried {
            *infra = false;
        }
        if (q58 && !retried && kind != Attribution::Environment) || decision::failed_round(kind) {
            return Ok(false);
        }
        let fact_identity = fact.identity();
        if prior.is_none() {
            let Evidence::CiFailed {
                head, completed_at, ..
            } = &fact.evidence
            else {
                unreachable!()
            };
            let mut hold = Hold {
                repository: fact.repository.clone(),
                number: fact.number,
                node,
                head: head.clone(),
                identity: fact_identity.clone(),
                context,
                failed_at: completed_at
                    .as_ref()
                    .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.with_timezone(&Utc)),
                since_ms: self.now_ms(),
                witness: None,
                witnessed_ms: None,
                retry: None,
                reporters: match &fact.evidence {
                    Evidence::CiFailed { details, .. } => details.reporters.clone(),
                    _ => vec![],
                },
            };
            // Catch-up can deliver T2 health before the delayed T1 failure.
            // Reconcile the journal, without another upstream read.
            for row in snapshot.rows.iter().filter(|row| row.table == "queue_gates") {
                let gate_fact: QueueFact = decode_stored(row.value.clone())?;
                if let Evidence::Gate(gate) = &gate_fact.evidence
                    && self.queue_hold_witness(snapshot, &gate_fact.repository, &mut hold, gate, commit)
                {
                    break;
                }
            }
            if hold.witness.is_none() {
                commit.rows.push(put("queue_holds", key(node, head), json!(hold)));
            }
        }
        commit
            .rows
            .push(put("queue_facts", fact_identity, json!(fact)));
        Ok(true)
    }
}
