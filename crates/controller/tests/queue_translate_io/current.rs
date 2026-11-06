use super::{Controller, Snapshot, World, Commit, Result, Error, NodeId, Event, Fact, check_run, put, commit_sha, looks, decode_stored, json, Value};
mod adapters {
    pub use super::super::adapters::prismabuild;
    pub mod mergify {
        use super::super::super::*;
        pub use super::super::boundary::{Evidence, QueueFact};
        pub use super::super::super::adapters::mergify::RequiredCheck;
        pub mod ci_evidence { use super::super::super::super::*; include!("ci-evidence-current.rs"); }
    }
}
mod boundary {
    use super::super::*;
    use super::adapters::mergify::RequiredCheck;
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct QueueFact { pub repository: Repository, pub number: u64, pub evidence: Evidence }
    include!("queue-fact-current.rs");
}
use boundary::{QueueFact, Evidence};
impl Evidence {
    pub(crate) fn outcome(&self) -> Option<tl3_controller::outcome::Outcome> {
        use tl3_controller::outcome::Outcome;
        Some(match self {
            Self::Merged { head, checks_passed } => Outcome::Merged { head: head.clone(), checks_passed: *checks_passed },
            Self::CiFailed { head, infra, details, .. } => Outcome::CiFailed {
                head: head.clone(), infra: *infra, failed_tests: details.failed_tests.clone(), evidence: details.clone(),
            },
            Self::Conflict { head, .. } => Outcome::Conflict { head: head.clone() },
            Self::PushByOther { head } => Outcome::PushByOther { head: head.clone() },
            Self::ClosedByPerson { .. } => Outcome::ClosedByPerson,
            _ => return None,
        })
    }
}

enum Observation { Queue(QueueFact), Outcome { node: NodeId, outcome: tl3_controller::outcome::Outcome, receipt: Option<String> } }
mod outcome { pub use tl3_controller::outcome::*; }

struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> {type Target=Controller;fn deref(&self)->&Controller { self.0 }}
impl std::ops::DerefMut for Shell<'_> {fn deref_mut(&mut self)->&mut Controller { self.0 }}
impl Shell<'_> {
    fn translate(&mut self, snapshot: &Snapshot, world: &World, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {
        let mut observation = observation;
        let mut queue_receipt = None;
        let event = loop {
        let event = match observation {
            Observation::Queue(mut fact) => {
                if let adapters::mergify::Evidence::Gate(gate) = &fact.evidence {
                    self.queue_gate(snapshot, &fact, gate, commit)?;
                    return Ok(None);
                }
                let identity = fact.identity();
                let Some(translation) =
                    tl3_controller_core::translate::begin(fact.recorded(snapshot)?)
                else {
                    return Ok(None);
                };
                let Some(node) = fact.node(snapshot)? else {
                    return Ok(None);
                };
                use adapters::mergify::Evidence;
                use tl3_controller_core::translate::{
                    self as core, Decision, IncidentAction, KernelKind as KernelFact,
                };
                use tl3_mergify::decide::translate as queue;
                let current = &world.nodes[node.0 as usize];
                let view = queue::NodeView {
                    phase: match current.phase {
                        tl3_kernel::Phase::Publishing => queue::Phase::Publishing,
                        tl3_kernel::Phase::Queued => queue::Phase::Queued,
                        tl3_kernel::Phase::Closed(_) => queue::Phase::Closed,
                        _ => queue::Phase::Other,
                    },
                    merged: current.merged.is_some(),
                    enqueue_pending: current.effect.is_some_and(|effect| {
                        core::enqueue_pending(
                            effect.kind == tl3_kernel::EffectKind::Enqueue,
                            effect.cancelling,
                        )
                    }),
                };
                // A closed node takes only a merge it has not recorded. A later
                // active enqueue must still be able to consume head evidence.
                let Some(kind) = fact.evidence.kind() else {
                    return Ok(None);
                };
                if !queue::admit(kind, view) {
                    return Ok(None);
                }
                if let Evidence::CiFailed {
                    head,
                    infra,
                    head_check: false,
                    details,
                    ..
                } = &mut fact.evidence
                {
                    *details = adapters::mergify::ci_evidence::bounded(details.clone());
                    let plan = check_run(snapshot, node, Some(head))?;
                    let outside = adapters::mergify::ci_evidence::outside_change(
                        &fact.repository,
                        details,
                        plan.as_ref(),
                    );
                    let previous: Vec<_> = snapshot
                        .rows
                        .iter()
                        .filter(|row| row.table == "queue_facts")
                        .map(|row| core::PriorFailure {
                            same_repository: row.value["repository"] == json!(&fact.repository),
                            same_number: row.value["number"] == fact.number,
                            same_head: row.value["evidence"]["CiFailed"]["head"] == *head,
                            infra: row.value["evidence"]["CiFailed"]["infra"] == true,
                            overlapping_test:
                                row.value["evidence"]["CiFailed"]["details"]["failed_tests"]
                                    .as_array()
                                    .is_some_and(|previous| {
                                        previous.iter().any(|previous| {
                                            details
                                                .failed_tests
                                                .iter()
                                                .any(|test| previous["test"] == test.test)
                                        })
                                    }),
                        })
                        .collect();
                    let repeated = core::repeated(&previous);
                    let (classified, decision) = queue::classify_ci(*infra, outside, repeated);
                    *infra = classified;
                    details.decision = Some(decision.as_str().into());
                }
                let q58 = matches!(&fact.evidence, Evidence::CiFailed { details, .. } if details.decision.as_deref() == Some("outside_change"));
                let current_failure = matches!(&fact.evidence, Evidence::CiFailed { head, .. } if commit_sha(snapshot, current.head) == Some(head));
                if current_failure
                    && self.queue_failure_cause(snapshot, &mut fact, node, q58, commit)?
                {
                    return Ok(None);
                }
                let value = json!(&fact);
                let head_current = match &fact.evidence {
                    Evidence::CiFailed { head, .. }
                    | Evidence::Conflict { head, .. }
                    | Evidence::DequeuedByCommand { head, .. } => core::head_current(
                        self.head(node, head)?.0,
                        world.nodes[node.0 as usize].head.map(|head| head.0),
                    ),
                    _ => true,
                };
                let approved_head = match &fact.evidence {
                    Evidence::CheckMissing { head, .. } | Evidence::QueueBlocked { head, .. } => {
                        commit_sha(snapshot, current.approved) == Some(head)
                    }
                    _ => false,
                };
                let route = match queue::route(
                    fact.evidence.kind().expect("queue evidence"),
                    view,
                    head_current,
                    approved_head,
                ) {
                    queue::Route::Record => core::Route::Record,
                    queue::Route::Skip => core::Route::Skip,
                    queue::Route::SetupIncident
                    | queue::Route::MissingIncident
                    | queue::Route::BlockedIncident
                    | queue::Route::ClearIncident => core::Route::Incident,
                    queue::Route::Kernel(kernel) => core::Route::Kernel(match kernel {
                        queue::KernelFact::PrMerged { checks_passed } => {
                            KernelFact::PrMerged { checks_passed }
                        }
                        queue::KernelFact::CiFailed { infra } => KernelFact::CiFailed { infra },
                        queue::KernelFact::Conflict => KernelFact::Conflict,
                        queue::KernelFact::PrDequeuedByPerson => KernelFact::PrDequeuedByPerson,
                        queue::KernelFact::PrClosedByPerson => KernelFact::PrClosedByPerson,
                        queue::KernelFact::PushByOther => KernelFact::PushByOther,
                    }),
                };
                let kind = match &fact.evidence {
                    Evidence::ChecksUnavailable { .. } => core::EvidenceKind::Setup,
                    Evidence::CheckMissing { .. } => core::EvidenceKind::Missing,
                    Evidence::QueueBlocked { .. } => core::EvidenceKind::Blocked,
                    Evidence::CheckReported { .. } => core::EvidenceKind::Reported,
                    Evidence::Merged { .. } => core::EvidenceKind::Merged,
                    Evidence::PushByOther { .. } => core::EvidenceKind::PushByOther,
                    _ => core::EvidenceKind::Other,
                };
                let kernel = match core::dispatch(translation, route, kind) {
                    Decision::Record => {
                        commit.rows.push(put("queue_facts", identity, value));
                        return Ok(None);
                    }
                    Decision::Skip => return Ok(None),
                    Decision::Kernel(kernel) => kernel,
                    // Setup rows (Q52, #447): none reaches the kernel, so no
                    // failure or credit is charged.
                    Decision::Incident(action) => {
                        match (action, fact.evidence) {
                            (
                                IncidentAction::Setup,
                                Evidence::ChecksUnavailable { permissions },
                            ) => {
                                self.check_setup_incident(snapshot, commit, node, &permissions)?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (
                                IncidentAction::Missing,
                                Evidence::CheckMissing { head, required },
                            ) => {
                                self.missing_check_incident(
                                    snapshot, commit, node, &head, &required,
                                )?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (
                                IncidentAction::Blocked,
                                Evidence::QueueBlocked {
                                    head,
                                    conditions,
                                    failed,
                                },
                            ) => {
                                self.queue_blocked_incident(
                                    snapshot,
                                    commit,
                                    node,
                                    &head,
                                    &conditions,
                                    &failed,
                                )?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (IncidentAction::Clear, Evidence::CheckReported { head, required }) => {
                                let id = looks::missing_check_incident_id(node, &head, &required);
                                let incidents: Vec<_> = snapshot
                                    .rows
                                    .iter()
                                    .map(|row| core::IncidentRow {
                                        own_incident: row.table == "incidents" && row.id == id,
                                        open: row.value["open"] == true,
                                    })
                                    .collect();
                                if let Some(index) = core::clear_reported(&incidents) {
                                    let mut incident: looks::Incident =
                                        decode_stored(snapshot.rows[index].value.clone())?;
                                    incident.open = false;
                                    commit.rows.push(put("incidents", id, json!(incident)));
                                }
                            }
                            _ => {
                                return Err(Error::Invalid(
                                    "only setup evidence routes to an incident",
                                ));
                            }
                        }
                        return Ok(None);
                    }
                    Decision::InvalidIncident => {
                        return Err(Error::Invalid("only setup evidence routes to an incident"));
                    }
                    Decision::InvalidKernel => {
                        return Err(Error::Invalid(
                            "queue evidence routed to another kernel fact",
                        ));
                    }
                };
                if let Some(outcome) = fact.evidence.outcome() {
                    queue_receipt = Some(put("queue_facts", identity, value));
                    observation = Observation::Outcome { node, outcome, receipt: None };
                    continue;
                }
                let fact = match kernel {
                    KernelFact::PrDequeuedByPerson => Fact::PrDequeuedByPerson,
                    _ => return Err(Error::Invalid("queue evidence routed to another kernel fact")),
                };
                commit.rows.push(put("queue_facts", identity, value));
                Event::Fact { node, fact }
            }
            Observation::Outcome { node, outcome, receipt } => {
                if receipt.as_ref().is_some_and(|id| snapshot.rows.iter().any(|row| {
                    matches!(row.table.as_str(), "delivery_outcomes" | "queue_facts") && row.id == *id
                })) { return Ok(None); }
                let recorded = json!({"node": node, "outcome": &outcome});
                use outcome::Outcome;
                let current = &world.nodes[node.0 as usize];
                // A closed request accepts only its first observed merge.
                if matches!(current.phase, tl3_kernel::Phase::Closed(_))
                    && (!matches!(outcome, Outcome::Merged { .. }) || current.merged.is_some())
                {
                    return Ok(None);
                }
                let fact = match outcome {
                    Outcome::Merged { head, checks_passed } => Fact::PrMerged {
                        head: self.head(node, &head)?, checks_passed,
                    },
                    Outcome::CiFailed { head, infra, .. } => {
                        if commit_sha(snapshot, current.head) != Some(head.as_str()) {
                            return Ok(None);
                        }
                        Fact::CiFailed { infra }
                    },
                    Outcome::Conflict { head } => {
                        if commit_sha(snapshot, current.head) != Some(head.as_str()) {
                            return Ok(None);
                        }
                        Fact::Conflict
                    },
                    Outcome::ClosedByPerson => Fact::PrClosedByPerson,
                    Outcome::PushByOther { head } => {
                        let commits: Vec<_> = snapshot
                            .interned
                            .iter()
                            .map(|item| tl3_controller_core::translate::CommitRow {
                                is_commit: item.kind == tl3_store::InternKind::Commit,
                                id: item.id,
                            })
                            .collect();
                        let previous_sha =
                            tl3_controller_core::translate::previous_commit(&commits, current.head.map(|head| head.0 as i64))
                                .map(|index| snapshot.interned[index].value.as_str());
                        let carried = tl3_controller_core::translate::carry_plan(
                            node.0,
                            &head,
                            check_run(snapshot, node, previous_sha)?,
                        );
                        if let Some(carried) = carried {
                            commit.rows.push(put(
                                "check_plans",
                                format!("{}:{}", carried.node, carried.head),
                                json!(carried.plan),
                            ));
                        }
                        Fact::PushByOther { head: self.head(node, &head)? }
                    },
                };
                if let Some(receipt) = receipt {
                    commit.rows.push(put("delivery_outcomes", receipt, recorded));
                }
                Event::Fact { node, fact }
            },

        };
        break event;
        };
        if let Some(receipt) = queue_receipt { commit.rows.push(receipt); }
        Ok(Some(event))
    }
}
pub(super) fn translate(controller: &mut Controller, snapshot: &Snapshot, world: &World, observation: super::Observation, commit: &mut Commit) -> Result<Option<Event>> {
    let observation = match observation { super::Observation::Queue(fact) => Observation::Queue(serde_json::from_value(json!(fact))?) };
    Shell(controller).translate(snapshot, world, observation, commit)
}

include!("cause-methods-current.rs");
