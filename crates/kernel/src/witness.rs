//! Q30/Q39 / #805: the real executable outside-event selector.
use super::*;
impl World {
    kernel_fn! {
    /// One outside event that moves the world closer to done: a successful
    /// reply, a confirmation, a fact, or one of Rob's answers. Kani proves it
    /// always exists while a node is open, and that it lowers `measure`.
    /// #771: the same witness branches, verified against their exact event keys.
    /// Node::next_command_id and the controller's checked steering constructors
    /// enforce the fresh-command scope. Raw kernel command exhaustion remains
    /// characterized by tests/progress_limits.rs, outside this theorem's scope.
    pub fn witness(&self) -> (result: Option<Event>) [
        requires world_checked(*self), fresh_commands(*self),
        ensures witness_selection(*self, result),
            unplanned_first_selection(*self, result),
    ] proof [reveal(world_checked); reveal(fresh_commands); reveal(witness_selection); reveal(outside_available); reveal(unplanned_first_selection);] {
        let mut i = 0usize;
        kernel_while! { (i < self.nodes.len()) [
            invariant i <= self.nodes.len(), self.nodes@.len() <= u64::MAX as int,
                world_checked(*self),
                (i > 0 && self.nodes@.len() > 0) ==> !(self.nodes@[0].phase == Phase::Unplanned
                    && progress_unplanned::obstructed(self.nodes@[0])),
                forall|j: int| 0 <= j < self.nodes@.len() ==> self.nodes@[j].last_cmd < u64::MAX,
                forall|j: int| 0 <= j < i ==> !outside_available(*self, j),
            decreases self.nodes.len() - i,
        ] {
            let n = &self.nodes[i];
            kernel_proof! {
                reveal(world_checked);
                dependencies::prerequisite_definition(*self, NodeId(i as u64));
            }
            if n.phase.is_closed() {
                i += 1;
                continue;
            }
            let node = NodeId(i as u64);
            if n.paused && !n.phase.is_stopping() {
                let event = Event::Command {
                    node,
                    id: n.next_command_id()?,
                    command: Command::Resume,
                };
                kernel_proof! { selected_event(*self, i as int, event); }
                return Some(event);
            }
            if let Some(e) = n.effect {
                let event = Event::Ack {
                    node,
                    seq: e.seq,
                    ack: Ack::Confirmed,
                };
                kernel_proof! { selected_event(*self, i as int, event); }
                return Some(event);
            }
            if n.waiting_incident {
                let event = Event::Fact {
                    node,
                    fact: Fact::IncidentCleared { by_rob: true },
                };
                kernel_proof! { selected_event(*self, i as int, event); }
                return Some(event);
            }
            if let Some(q) = n.question {
                let answer = match q.kind {
                    QKind::AuditDisagreement | QKind::EffectFailed => Answer::Continue,
                    QKind::NeedsSplitting => Answer::Stop,
                    QKind::CantMeet => Answer::TryAgain,
                    QKind::OutOfCredit => Answer::AddCredit(1),
                    QKind::CleanupFailed => Answer::MarkHandled,
                };
                let command = Command::Answer {
                    question: q.id,
                    answer,
                };
                let event = Event::Command {
                    node,
                    id: n.next_command_id()?,
                    command,
                };
                kernel_proof! { selected_event(*self, i as int, event); }
                return Some(event);
            }
            if n.phase == Phase::Unplanned && !self.dependencies_delivered(node) {
                let dependency = self.first_undelivered(node).unwrap();
                if dependency.rejected || dependency.status == DependencyStatus::Abandoned {
                    let event = Event::Command {
                        node,
                        id: n.next_command_id()?,
                        command: Command::ChangeRequest,
                    };
                    kernel_proof! { selected_event(*self, i as int, event); }
                    return Some(event);
                }
                let event = Event::Fact {
                    node,
                    fact: Fact::Dependency {
                        target: dependency.target,
                        status: DependencyStatus::Delivered,
                    },
                };
                kernel_proof! { selected_event(*self, i as int, event); }
                return Some(event);
            }
            match n.current() {
                Some(Item::Await) => {
                    let generation = n.generation;
                    let reply = match n.phase {
                        Phase::Ingesting => Reply::IngestDone { size: 1 },
                        Phase::Auditing => Reply::AuditAgrees,
                        Phase::Planning { must_split: true } => Reply::PlanSplit,
                        Phase::Planning { must_split: false } => Reply::PlanLeaf {
                            footprint: if !n.footprint.is_empty() {
                                n.footprint
                            } else {
                                Footprint::inline(1)
                            },
                            fits: true,
                        },
                        Phase::Making => Reply::MakerDone {
                            head: n.head.unwrap_or(HeadId(0)),
                        },
                        Phase::Checking => Reply::ChecksDone {
                            report: true,
                            pass: true,
                            in_footprint: true,
                        },
                        Phase::Reviewing => Reply::Approved {
                            head: n.head.unwrap_or(HeadId(0)),
                        },
                        Phase::Rebasing => Reply::RebaseClean {
                            head: n.head.unwrap_or(HeadId(0)),
                        },
                        _ => { i += 1; continue; },
                    };
                    let event = Event::Reply {
                        node,
                        generation,
                        reply,
                    };
                    kernel_proof! { selected_event(*self, i as int, event); }
                    return Some(event);
                }
                Some(Item::Merge) => {
                    let head = n.head.unwrap_or(HeadId(0));
                    let fact = Fact::PrMerged {
                        head,
                        checks_passed: true,
                    };
                    let event = Event::Fact { node, fact };
                    kernel_proof! { selected_event(*self, i as int, event); }
                    return Some(event);
                }
                // Waiting on the gate or the pool: another node holds what it needs.
                _ => {},
            }
            i += 1;
        } }
        None
    }
    }
}
