use super::*;

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub node: NodeId,
    pub target: DependencyTarget,
    pub status: DependencyStatus,
    /// A rejected declaration blocks its owner without adding an edge to the graph.
    pub rejected: bool,
}

impl World {
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    pub fn dependencies_delivered(&self, node: NodeId) -> bool {
        self.dependencies
            .iter()
            .filter(|d| d.node == node)
            .all(|d| !d.rejected && d.status == DependencyStatus::Delivered)
    }

    fn dependency_path(&self, from: NodeId, to: NodeId) -> bool {
        if self.nodes.len() <= 64 {
            // Most worlds fit in a bitset. Repeated edge relaxation reaches
            // the same nodes as the unbounded traversal below, without allocation.
            let mut seen = 1u64 << from.0;
            loop {
                if seen & (1u64 << to.0) != 0 {
                    return true;
                }
                let before = seen;
                for dependency in &self.dependencies {
                    if !dependency.rejected
                        && dependency.status == DependencyStatus::Pending
                        && !self.nodes[dependency.node.0 as usize].phase.is_closed()
                        && seen & (1u64 << dependency.node.0) != 0
                        && let Some(target) = dependency.target.request
                    {
                        seen |= 1u64 << target.0;
                    }
                }
                if seen == before {
                    return false;
                }
            }
        }
        #[cfg(kani)]
        {
            // Proof worlds have at most three roots. Prove this before pruning
            // the heap traversal, as for interned footprints.
            assert!(false, "dependency proof entered the large-world traversal");
            kani::assume(false);
        }
        let mut seen = alloc::vec![false; self.nodes.len()];
        let mut pending = alloc::vec![from];
        while let Some(node) = pending.pop() {
            if node == to {
                return true;
            }
            let Some(index) = self.index(node) else {
                continue;
            };
            if seen[index] || self.nodes[index].phase.is_closed() {
                continue;
            }
            seen[index] = true;
            for dependency in &self.dependencies {
                if dependency.node == node
                    && !dependency.rejected
                    && dependency.status == DependencyStatus::Pending
                    && let Some(target) = dependency.target.request
                {
                    pending.push(target);
                }
            }
        }
        false
    }

    pub fn dependency_graph_acyclic(&self) -> bool {
        self.dependencies.iter().all(|d| {
            self.nodes[d.node.0 as usize].phase.is_closed()
                || d.rejected
                || d.status != DependencyStatus::Pending
                || d.target
                    .request
                    .is_none_or(|target| !self.dependency_path(target, d.node))
        })
    }

    pub(super) fn check_dependencies(&self) -> Result<(), Broken> {
        for (index, node) in self.nodes.iter().enumerate() {
            if node.credit.withheld
                && !self
                    .dependencies
                    .iter()
                    .any(|d| d.node == NodeId(index as u64))
            {
                return Err(Broken::Dependencies);
            }
        }
        if self.dependencies.is_empty() {
            return Ok(());
        }
        for (index, dependency) in self.dependencies.iter().enumerate() {
            let Some(owner) = self.index(dependency.node) else {
                return Err(Broken::Dependencies);
            };
            if dependency.target.number == 0
                || dependency.target.request.is_some_and(|target| {
                    self.index(target)
                        .is_none_or(|i| self.nodes[i].repo != self.nodes[owner].repo)
                })
                || self.dependencies[..index].iter().any(|d| {
                    d.node == dependency.node
                        && d.target.kind == dependency.target.kind
                        && d.target.number == dependency.target.number
                })
            {
                return Err(Broken::Dependencies);
            }
            if (dependency.rejected || dependency.status != DependencyStatus::Delivered)
                && (self.nodes[owner].planning_started
                    || self.nodes[owner].question.is_some_and(|q| {
                        !matches!(
                            q.cont,
                            Cont::Ingesting
                                | Cont::Auditing
                                | Cont::Unplanned
                                | Cont::Stop(_)
                                | Cont::Nowhere
                        )
                    }))
            {
                return Err(Broken::Dependencies);
            }
            if (dependency.rejected || dependency.status != DependencyStatus::Delivered)
                && !matches!(
                    self.nodes[owner].phase,
                    Phase::Ingesting
                        | Phase::Auditing
                        | Phase::Unplanned
                        | Phase::AwaitingRob
                        | Phase::Stopping(_)
                        | Phase::Closed(_)
                )
            {
                return Err(Broken::Dependencies);
            }
            if !self.dependencies_delivered(dependency.node)
                && matches!(
                    self.nodes[owner].phase,
                    Phase::Ingesting | Phase::Auditing | Phase::Unplanned
                )
                && self.nodes[owner].credit.balance() != 0
            {
                return Err(Broken::Dependencies);
            }
            if self.nodes[owner].phase == Phase::Unplanned
                && !self.dependencies_delivered(dependency.node)
            {
                let n = &self.nodes[owner];
                if n.holds
                    || n.pool
                    || n.ticket.is_some()
                    || n.run.is_some()
                    || n.credit.balance() != 0
                {
                    return Err(Broken::Dependencies);
                }
            }
        }
        if !self.dependency_graph_acyclic() {
            return Err(Broken::Dependencies);
        }
        Ok(())
    }

    pub(super) fn withhold_dependency_credit(&mut self, i: usize) {
        if !self.dependencies_delivered(NodeId(i as u64))
            && !self.nodes[i].planning_started
            && matches!(
                self.nodes[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            )
        {
            let n = &mut self.nodes[i];
            if n.credit.ready {
                n.credit.withheld = true;
            }
        }
    }

    pub(super) fn reset_dependencies(&mut self, i: usize) {
        if self.nodes[i].phase != Phase::Ingesting || self.nodes[i].planning_started {
            return;
        }
        self.dependencies.retain(|d| d.node != NodeId(i as u64));
        let n = &mut self.nodes[i];
        n.waiting_prerequisite = None;
        n.credit.withheld = false;
    }

    pub(super) fn dependency_fact(
        &mut self,
        i: usize,
        target: DependencyTarget,
        status: DependencyStatus,
        out: &mut Outbox,
    ) {
        if target.number == 0
            || target.request.is_some_and(|node| {
                self.index(node)
                    .is_none_or(|j| self.nodes[i].repo != self.nodes[j].repo)
            })
        {
            return;
        }
        let node = NodeId(i as u64);
        let existing = self.dependencies.iter().position(|d| {
            d.node == node && d.target.kind == target.kind && d.target.number == target.number
        });
        if let Some(index) = existing {
            let previous = self.dependencies[index];
            if previous.rejected || previous.status != DependencyStatus::Pending {
                return;
            }
            if status == DependencyStatus::Pending && target.request == previous.target.request {
                return;
            }
        } else if self.nodes[i].planning_started
            || !matches!(
                self.nodes[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            )
        {
            return;
        }
        let rejected = status == DependencyStatus::Pending
            && target
                .request
                .is_some_and(|target| self.dependency_path(target, node));
        let dependency = Dependency {
            node,
            target,
            status,
            rejected,
        };
        match existing {
            Some(index) => self.dependencies[index] = dependency,
            None => self.dependencies.push(dependency),
        }
        if rejected {
            notice(
                out,
                i,
                Notice::Note(NoteKind::DependencyCycle {
                    number: target.number,
                }),
            );
        } else if status == DependencyStatus::Abandoned {
            notice(
                out,
                i,
                Notice::Note(NoteKind::DependencyAbandoned {
                    number: target.number,
                }),
            );
        }
        self.nodes[i].waiting_prerequisite = self
            .dependencies
            .iter()
            .find(|d| d.node == node && (d.rejected || d.status != DependencyStatus::Delivered))
            .map(|d| d.target.number);
        self.withhold_dependency_credit(i);
    }
}
