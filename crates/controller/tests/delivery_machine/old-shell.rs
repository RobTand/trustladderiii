impl Controller {
    fn retire_effect(&mut self, snapshot: &Snapshot, row: &tl3_store::WorldRow) -> Result<()> {
        let delivery: Delivery = decode_stored(row.value.clone())?;
        if executor::upstream(delivery.output) == executor::Upstream::Delivery
            || approval_delivery::is_approval(&delivery)
        {
            self.adapters
                .adapter(&delivery)?
                .retire_delivery(&delivery)?;
        }
        let mut commit = Commit::new(
            format!("retire:{}:{}", row.id, snapshot.revision.get()),
            json!({"retired": row.id}),
            snapshot,
        );
        commit.rows.push(RowChange::Delete {
            table: row.table.clone(),
            id: row.id.clone(),
        });
        self.store.commit(&commit)?;
        Ok(())
    }

    fn observe_pending_effect(&mut self, delivery: &Delivery) -> Result<bool> {
        let before = self.store.load()?.revision;
        // Dispatch and reconciliation retain their existing error/allowance handling.
        if let Ok(observations) = self.adapters.adapter(delivery)?.observations(delivery) {
            for (index, observation) in observations.into_iter().enumerate() {
                let key = format!(
                    "pending:{}:{index}:{}",
                    delivery.key,
                    evidence_key(&observation)?
                );
                self.observe(&key, observation)?;
            }
        }
        Ok(self.store.load()?.revision != before)
    }

    fn transition(&mut self, effect: &Effect, to: EffectState) -> Result<()> {
        let snapshot = self.store.load()?;
        let delivery = hydrate(effect, &snapshot)?;
        let mut commit = Commit::new(
            format!("delivery:{}:{}", effect.key, snapshot.revision.get()),
            json!({"effect": effect.key, "from": effect.state, "to": to}),
            &snapshot,
        );
        commit.transitions.push(EffectTransition {
            key: effect.key.clone(),
            from: effect.state,
            to,
        });
        if to == EffectState::Sent && approval_delivery::is_approval(&delivery) {
            commit.rows.push(RowChange::Delete {
                table: approval_delivery::AUTHORIZATIONS.into(),
                id: effect.key.clone(),
            });
        }
        // A pending-checks hold waits on check webhooks; it arms no silence timer.
        if to == EffectState::Uncertain
            && let Some(class) = self
                .held_effects
                .get(&executor::upstream(delivery.output))
                .and_then(|effects| effects.get(&effect.key))
            && class != looks::CHECKS_PENDING
        {
            self.failure_allowance(&snapshot, &effect.key, class, &mut commit)?;
        }
        self.store.commit(&commit)?;
        Ok(())
    }

    fn cancel_absent(&mut self, effect: &Effect, _delivery: &Delivery) -> Result<()> {
        self.submit_kernel_input(
            format!("cancel:{}:{}", effect.key, effect.updated_revision.get()),
            input_errors::PendingKernelInput::CancelAbsent {
                key: effect.key.clone(),
            },
        )
        .map(|_| ())
    }

    fn prepare_cancel_absent(
        &mut self,
        snapshot: &Snapshot,
        effect: &Effect,
        delivery: &Delivery,
    ) -> Result<Commit> {
        let mut commit = Commit::new(
            format!("cancel:{}:{}", effect.key, snapshot.revision.get()),
            json!({"cancelled_absent": effect.key}),
            snapshot,
        );
        commit.transitions.push(EffectTransition {
            key: effect.key.clone(),
            from: effect.state,
            to: EffectState::Cancelled,
        });
        if let DeliveryOutput::Effect { node, seq, .. } = delivery.output {
            self.apply(
                snapshot,
                snapshot::decode(snapshot)?,
                Event::Ack {
                    node,
                    seq,
                    ack: Ack::Retryable,
                },
                &mut commit,
            )?;
        }
        if approval_delivery::is_approval(delivery) {
            queue_retirement(&mut commit, delivery);
        }
        looks::resolve_effect_incidents(snapshot, &mut commit, |_, key| Ok(key == delivery.key))?;
        Ok(commit)
    }

    fn complete(&mut self, key: &str, completion: Completion) -> Result<()> {
        let snapshot = self.store.load()?;
        let effect = snapshot
            .pending_effects
            .iter()
            .find(|e| e.key == key)
            .ok_or(Error::Invalid("completion has no pending effect"))?;
        self.submit_kernel_input(
            format!("completion:{key}:{}", effect.updated_revision.get()),
            input_errors::PendingKernelInput::Completion {
                key: key.into(),
                completion,
            },
        )
        .map(|_| ())
    }

    fn prepare_completion(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        mut completion: Completion,
    ) -> Result<Commit> {
        let effect = snapshot
            .pending_effects
            .iter()
            .find(|e| e.key == key)
            .ok_or(Error::Invalid("completion has no pending effect"))?;
        let delivery: Delivery = decode_stored(effect.payload.clone())?;
        if completion.ack == Ack::DeliveryAccepted && !matches!(delivery.output,
            DeliveryOutput::Effect { kind: tl3_kernel::EffectKind::Enqueue, .. })
        {
            return Err(Error::Invalid("delivery acceptance requires the handoff effect"));
        }
        if completion.ack == Ack::Confirmed
            && effect.state != EffectState::Cancelling
            && matches!(delivery.output, DeliveryOutput::Effect {
                kind: tl3_kernel::EffectKind::Enqueue, ..
            })
        {
            return Err(Error::Invalid("live handoff requires typed delivery acceptance"));
        }
        if completion.ack == Ack::Permanent
            && publication_failure::handles(&delivery)
            && completion.diagnostic.is_none()
        {
            completion.diagnostic = Some(publication_failure::Diagnostic::capture(
                &delivery,
                None,
                completion.failure_class.as_deref().unwrap_or("unknown"),
            )?);
        }
        let mut commit = Commit::new(
            format!("completion:{key}:{}", effect.updated_revision.get()),
            json!(&completion),
            snapshot,
        );
        let stopped_node = match delivery.output {
            executor::Output::Effect {
                node,
                kind: tl3_kernel::EffectKind::StopRun,
                ..
            } if completion.ack == Ack::Confirmed => Some(node),
            _ => None,
        };
        self.release_terminal_model_probes(snapshot, &mut commit, stopped_node)?;
        self.retire_completed_model_attempts(snapshot, &mut commit)?;
        if let Some(diagnostic) = &completion.diagnostic {
            eprintln!("{}", diagnostic.render());
            commit
                .rows
                .push(put("publication_failures", key.into(), json!(diagnostic)));
        }
        // A confirmed completion may still name its failure, such as a check
        // action whose trusted harness rejected TL3's own input.
        if let Some(class) = completion.failure_class.as_deref() {
            self.failure_allowance(snapshot, key, class, &mut commit)?;
        }
        self.accepted_delivery_refusal(snapshot, &delivery, &completion, &mut commit)?;
        if effect.state == EffectState::Uncertain && !matches!(completion.ack, Ack::Confirmed | Ack::DeliveryAccepted) {
            // A terminal failed attempt was found. Return to the sending state
            // through the effect machine before recording its failure.
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Uncertain,
                to: EffectState::Intended,
            });
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Sent,
            });
        }
        let from = if effect.state == EffectState::Uncertain && !matches!(completion.ack, Ack::Confirmed | Ack::DeliveryAccepted) {
            EffectState::Sent
        } else {
            effect.state
        };
        let state = if effect.state == EffectState::Cancelling {
            EffectState::Confirmed
        } else {
            match completion.ack {
                Ack::Confirmed | Ack::DeliveryAccepted => EffectState::Confirmed,
                Ack::Retryable => EffectState::Intended,
                Ack::Permanent | Ack::BodyPolicyRefused => EffectState::Failed,
            }
        };
        commit.transitions.push(EffectTransition {
            key: key.into(),
            from,
            to: state,
        });
        // Preserve teardown evidence in the same transaction that can emit the summary.
        for observation in &completion.observations {
            match observation {
                Observation::ScratchSaved { node, reference } => {
                    let value = json!({"node":node,"reference":reference});
                    commit
                        .rows
                        .push(put("saved_scratch", reference.clone(), value));
                    commit.rows.push(put(
                        "scratch_evidence",
                        reference.clone(),
                        json!({"node":node,"reference":reference,"status":"saved"}),
                    ));
                }
                Observation::ScratchPruned { reference } => {
                    if let Some(saved) = snapshot
                        .rows
                        .iter()
                        .find(|r| r.table == "saved_scratch" && r.id == *reference)
                    {
                        let mut value = saved.value.clone();
                        value["status"] = json!("pruned");
                        commit
                            .rows
                            .push(put("scratch_evidence", reference.clone(), value));
                    }
                    commit.rows.push(RowChange::Delete {
                        table: "saved_scratch".into(),
                        id: reference.clone(),
                    });
                }
                _ => {}
            }
        }
        if let DeliveryOutput::Effect {
            node, seq, kind, ..
        } = delivery.output
        {
            let ack = if kind == tl3_kernel::EffectKind::OpenPr
                && completion.ack == Ack::Permanent
                && completion
                    .diagnostic
                    .as_ref()
                    .is_some_and(publication_failure::Diagnostic::is_body_refusal)
            {
                Ack::BodyPolicyRefused
            } else {
                completion.ack
            };
            self.apply(
                snapshot,
                snapshot::decode(snapshot)?,
                Event::Ack { node, seq, ack },
                &mut commit,
            )?;
        }
        if completion.ack == Ack::Permanent && delivery.is_postmortem() {
            // #403: a postmortem never posts or raises an incident; it fails.
            self.postmortem_failed(snapshot, &delivery, "launch_failed", &mut commit)?;
        } else if completion.ack == Ack::Permanent
            && !matches!(delivery.output, DeliveryOutput::Effect { .. })
        {
            self.failed_notice(
                snapshot,
                &delivery,
                completion
                    .failure_class
                    .as_deref()
                    .unwrap_or("permanent_failure"),
                &mut commit,
            )?;
        }
        if completion.ack == Ack::Retryable
            && state == EffectState::Intended
            && matches!(delivery.output, DeliveryOutput::Effect { .. })
        {
            // The kernel assigns a fresh sequence to its retry. Retire the old
            // delivery and its watch together, so absence of the cancelled key
            // cannot fail the replacement operation in the same generation.
            commit.rows.push(RowChange::Delete {
                table: "watches".into(),
                id: key.into(),
            });
            if approval_delivery::is_approval(&delivery) {
                for replacement in &commit.effects {
                    let next: Delivery = decode_stored(replacement.payload.clone())?;
                    if approval_delivery::is_approval(&next) {
                        commit.rows.push(put(
                            approval_delivery::AUTHORIZATIONS,
                            next.key.clone(),
                            json!(true),
                        ));
                        commit.rows.push(put(
                            approval_delivery::LINEAGE_SAFE,
                            next.key,
                            json!(true),
                        ));
                    }
                }
            }
            queue_retirement(&mut commit, &delivery);
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Cancelled,
            });
        }
        if matches!(completion.ack, Ack::Confirmed | Ack::DeliveryAccepted) {
            if matches!(
                delivery.output,
                DeliveryOutput::Effect {
                    kind: tl3_kernel::EffectKind::StartRun(_),
                    ..
                }
            ) {
                commit.rows.push(RowChange::Delete {
                    table: "worker_input_contexts".into(),
                    id: delivery.node().0.to_string(),
                });
            }
            looks::resolve_effect_incidents(snapshot, &mut commit, |_, effect| Ok(effect == key))?;
            if let Some(marker) = &delivery.marker_key {
                commit.rows.push(RowChange::Delete {
                    table: "notice_incidents".into(),
                    id: marker.clone(),
                });
            }
        }
        // Upstream facts remain evidence even when the attempted write was refused.
        for (index, observation) in completion.observations.iter().enumerate() {
            if matches!(
                observation,
                Observation::ScratchSaved { .. } | Observation::ScratchPruned { .. }
            ) {
                continue;
            }
            self.queue_observation(snapshot, &mut commit, key, index, observation)?;
        }
        if approval_delivery::is_approval(&delivery)
            && matches!(
                state,
                EffectState::Confirmed | EffectState::Failed | EffectState::Cancelled
            )
        {
            queue_retirement(&mut commit, &delivery);
        }
        evidence::retire_delivered(snapshot, &mut commit, &delivery, completion.ack);
        Ok(commit)
    }

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
            commit
                .rows
                .push(put(LINEAGE_SAFE, effect.key.clone(), json!(true)));
        }
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
                && copies as u64 >= self.store.approval_dispatch_count(&row.id)? as u64
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
