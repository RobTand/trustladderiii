//! Production consistency and settledness queries, with their safety contracts.
use super::*;

impl World {
    kernel_fn! {
    /// #771: true from the actual settle probe excludes every sufficient
    /// internal move used by the gate and pool obstruction argument.
    pub fn is_settled(&self) -> (result: bool) [
        requires settle_ready(*self),
        ensures result ==> settled_world(*self),
    ] proof [reveal(settled_world);] {
        let mut w = snapshot_world(self);
        let mut out = Outbox::new();
        kernel_proof! { reveal(empty_outbox); }
        let mut i = 0usize;
        kernel_while! { (i < w.nodes.len()) [
            invariant i <= w.nodes.len(), settle_ready(w), out.len == 0,
                w.nodes@ == self.nodes@, w.gates@ == self.gates@,
                w.pool_capacity == self.pool_capacity, w.dependencies@ == self.dependencies@,
                forall|j: int| 0 <= j < i ==> !movable(*self, j),
            decreases w.nodes.len() - i,
        ] {
            kernel_proof! {
                potential_member(w.nodes@, i as int, w.nodes@.len(), |n: Node| ticket_capacity(n));
            }
            kernel_snapshot! { before_probe = w }
            if advance(&mut w, i, &mut out) { return false; }
            kernel_proof! { movable_frame(*self, before_probe@, i as int); }
            i += 1;
        } }
        true
    }
    }

    kernel_fn! {
    /// #771: successful executable consistency checking supplies the exact
    /// progress invariants used by the source-connected theorem.
    pub fn consistent(&self) -> (result: bool) [
        requires world_headroom(*self), storage_ordered(*self),
        ensures result ==> world_checked(*self),
    ] proof [reveal(storage_ordered);] {
        self.check().is_ok()
    }
    }

    kernel_fn! {
    /// #771: the real world checker establishes positive capacities and the
    /// per-node and prerequisite facts, without changing any acceptance rule.
    pub fn check(&self) -> (result: Result<(), Broken>) [
        requires world_headroom(*self), storage_ordered(*self),
        ensures result.is_ok() ==> world_checked(*self),
    ] proof [reveal(world_headroom); reveal(world_checked); reveal(storage_ordered);] {
        self.check_dependencies()?;
        if self.gates.is_empty() || self.pool_capacity == 0 {
            return Err(Broken::Config);
        }
        let mut gate_index = 0usize;
        kernel_while! { (gate_index < self.gates.len()) [
            invariant gate_index <= self.gates.len(),
                forall|j: int| 0 <= j < gate_index ==> self.gates@[j].w > 0,
            decreases self.gates.len() - gate_index,
        ] {
            if self.gates[gate_index].w == 0 { return Err(Broken::Config); }
            gate_index += 1;
        } }
        let mut i = 0;
        kernel_while! { (i < self.nodes.len()) [
            invariant i <= self.nodes.len(),
                forall|j: int| 0 <= j < i ==> node_checked(*self, self.nodes@[j]),
            decreases self.nodes.len() - i,
        ] {
            self.check_node(&self.nodes[i])?;
            i += 1;
        } }
        // Locks are the footprints of holders in the same repo.
        let mut i = 0;
        kernel_while! { (i < self.nodes.len()) [
            invariant i <= self.nodes.len(),
                forall|j: int| 0 <= j < self.nodes@.len() ==> node_checked(*self, self.nodes@[j]),
            decreases self.nodes.len() - i,
        ] {
            let n = self.nodes[i];
            if n.holds {
                let mut j = 0;
                kernel_while! { (j < i) [
                    invariant j <= i < self.nodes.len(),
                        forall|k: int| 0 <= k < self.nodes@.len() ==> node_checked(*self, self.nodes@[k]),
                    decreases i - j,
                ] {
                    let other = self.nodes[j];
                    kernel_proof! {
                        assert(node_checked(*self, n));
                        assert(node_checked(*self, other));
                    }
                    if other.holds
                        && other.repo == n.repo
                        && !self.footprints_disjoint(other.footprint, n.footprint)
                    {
                        return Err(Broken::Gate);
                    }
                    j += 1;
                } }
            }
            i += 1;
        } }
        let mut r = 0usize;
        kernel_while! { (r < self.gates.len()) [
            invariant r <= self.gates.len(),
            decreases self.gates.len() - r,
        ] {
            let mut count = 0usize;
            let mut index = 0usize;
            kernel_while! { (index < self.nodes.len()) [
                invariant index <= self.nodes.len(), count <= index,
                decreases self.nodes.len() - index,
            ] {
                let n = &self.nodes[index];
                if n.holds && n.repo == RepoId(r as u64) { count += 1; }
                index += 1;
            } }
            if count > self.gates[r].w as usize { return Err(Broken::Gate); }
            r += 1;
        } }
        let mut runs = 0u32;
        let mut index = 0usize;
        kernel_while! { (index < self.nodes.len()) [
            invariant index <= self.nodes.len(), runs as int <= open_prefix(self.nodes@, index as nat),
                open_prefix(self.nodes@, self.nodes@.len()) <= u32::MAX as int,
                forall|j: int| 0 <= j < self.nodes@.len() ==> node_checked(*self, self.nodes@[j]),
            decreases self.nodes.len() - index,
        ] {
            kernel_proof! {
                reveal(open_prefix);
                open_prefix_monotone(self.nodes@, index as nat + 1, self.nodes@.len());
            }
            if self.nodes[index].pool { runs += 1; }
            index += 1;
        } }
        if runs > self.pool_capacity as u32 { return Err(Broken::Pool); }
        let mut a = 0usize;
        kernel_while! { (a < self.nodes.len()) [
            invariant a <= self.nodes.len(),
            decreases self.nodes.len() - a,
        ] {
            let x = &self.nodes[a];
            let mut b = 0usize;
            kernel_while! { (b < self.nodes.len()) [
                invariant a < self.nodes.len(), b <= self.nodes.len(),
                decreases self.nodes.len() - b,
            ] {
                let y = &self.nodes[b];
                if a != b && x.ticket.is_some() && x.ticket == y.ticket {
                    return Err(Broken::Ticket);
                }
                b += 1;
            } }
            a += 1;
        } }
        Ok(())
    }
    }

    kernel_fn! {
    /// #771: the production checker establishes the progress proof's node
    /// invariants. This is a consequence of its existing checks, not a new rule.
    #[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
    fn check_node(&self, n: &Node) -> (result: Result<(), Broken>) [
        ensures result.is_ok() ==> node_checked(*self, *n),
    ] {
        let _gate_count = self.gates.len();
        let items = agenda(n.phase);
        if match n.repo.index() { Some(r) => r >= self.gates.len(), None => true } {
            return Err(Broken::Config);
        }
        if !self.has_footprint(n.footprint) {
            return Err(Broken::Footprint);
        }
        // Only a stop for TL3 review records its step (Q54).
        if n.review_stop.is_some() && !n.phase.is_stopping() && !n.phase.is_closed() {
            return Err(Broken::ReviewStop);
        }
        if n.phase.is_closed() {
            if n.pos != 0 {
                return Err(Broken::Agenda);
            }
        } else if n.pos as usize > items.len()
            || (n.pos as usize == items.len()
                && !matches!(n.phase, Phase::Publishing | Phase::Stopping(_)))
        {
            // Mid-settle, Publishing and Stopping can sit at the end of their agenda for a moment.
            return Err(Broken::Agenda);
        }
        let current = n.current();

        // One effect in flight, with a fresh key, belonging to the current step
        // unless the node has moved on.
        if let Some(e) = n.effect {
            if e.seq >= n.next_seq {
                return Err(Broken::Effect);
            }
            if !e.cancelling {
                let fits = if e.kind.starts_operation() {
                    current == Some(Item::Await)
                } else {
                    current == Some(Item::Do(e.kind)) || current == Some(Item::Teardown(e.kind))
                };
                if !fits {
                    return Err(Broken::Effect);
                }
            }
        }

        // Slots and files.
        if n.phase.holding() && !n.holds {
            return Err(Broken::Holds);
        }
        if n.holds && !(n.phase.holding() || (n.phase.is_stopping() && n.pos <= RELEASE_STEP)) {
            return Err(Broken::Holds);
        }

        // Runs, jobs and the agent pool.
        let run_phase = matches!(
            n.phase,
            Phase::Ingesting
                | Phase::Auditing
                | Phase::Planning { .. }
                | Phase::Making
                | Phase::Reviewing
                | Phase::Rebasing
        );
        if current == Some(Item::Await) {
            let mine = if n.phase == Phase::Checking {
                n.job
            } else {
                n.run
            };
            if mine != Some(n.generation) {
                return Err(Broken::Run);
            }
        }
        if let Some(g) = n.run {
            let awaiting = run_phase && current == Some(Item::Await) && g == n.generation;
            if !(awaiting || (n.phase.is_stopping() && n.pos == 0)) {
                return Err(Broken::Run);
            }
        }
        if let Some(g) = n.job {
            let awaiting =
                n.phase == Phase::Checking && current == Some(Item::Await) && g == n.generation;
            if !(awaiting || (n.phase.is_stopping() && n.pos <= 1)) {
                return Err(Broken::Run);
            }
        }
        if n.pool && (n.run.is_none() || n.phase == Phase::Rebasing) {
            return Err(Broken::Pool);
        }

        // Places in line.
        if let Some(t) = n.ticket {
            let in_line = matches!(current, Some(Item::Slot) | Some(Item::Start(_)));
            if t >= self.next_ticket
                || n.waiting_incident
                || n.effect.is_some()
                || n.pool
                || !in_line
            {
                return Err(Broken::Ticket);
            }
        }

        // Questions to Rob.
        match n.question {
            Some(q) => {
                let price = match q.kind {
                    QKind::CantMeet | QKind::OutOfCredit => 1,
                    _ => 0,
                };
                if q.id >= self.next_question || q.cost != price || (q.cost > 0 && !n.credit.ready)
                {
                    return Err(Broken::Question);
                }
                // While Rob is asked, nothing else is in flight for the node.
                if matches!(n.effect, Some(e) if !e.cancelling) {
                    return Err(Broken::Question);
                }
                match n.phase {
                    Phase::AwaitingRob => {
                        // Each kind of question carries only the continuations the kernel gives it.
                        let fits = match (q.kind, q.cont) {
                            (QKind::AuditDisagreement, Cont::Unplanned) => true,
                            (QKind::AuditDisagreement, Cont::Stop(o)) => matches!(
                                o,
                                Outcome::Duplicate | Outcome::AlreadyDone | Outcome::NotActionable
                            ),
                            (QKind::NeedsSplitting, Cont::Nowhere) => true,
                            (
                                QKind::CantMeet,
                                Cont::Planning { .. } | Cont::Ready(Resume::Making),
                            ) => true,
                            (
                                QKind::OutOfCredit,
                                Cont::Planning { .. }
                                | Cont::Ready(Resume::Making)
                                | Cont::Ready(Resume::Checking)
                                | Cont::Ready(Resume::Reviewing),
                            ) => true,
                            (
                                QKind::EffectFailed,
                                Cont::Ingesting
                                | Cont::Auditing
                                | Cont::Unplanned
                                | Cont::Planning { .. }
                                | Cont::Ready(_),
                            ) => true,
                            _ => false,
                        };
                        if !fits {
                            return Err(Broken::Question);
                        }
                        if let Cont::Ready(r) = q.cont {
                            self.check_ready(n, r)?;
                        }
                        if matches!(
                            q.cont,
                            Cont::Auditing | Cont::Unplanned | Cont::Planning { .. }
                        ) && !n.credit.ready
                        {
                            return Err(Broken::Question);
                        }
                    }
                    Phase::Stopping(_) => {
                        if q.kind != QKind::CleanupFailed
                            || !matches!(current, Some(Item::Teardown(_)))
                        {
                            return Err(Broken::Question);
                        }
                    }
                    _ => return Err(Broken::Question),
                }
            }
            None => {
                if n.phase == Phase::AwaitingRob {
                    return Err(Broken::Question);
                }
            }
        }
        if n.phase == Phase::AwaitingRob && (n.run.is_some() || n.job.is_some() || n.pool) {
            return Err(Broken::Question);
        }

        // Incidents.
        if n.run_failures > 1 || n.fx_failures > 1 || n.run_incidents > 2 || n.fx_incidents > 2 {
            return Err(Broken::Incident);
        }
        if n.incident_needs_rob && !n.waiting_incident {
            return Err(Broken::Incident);
        }
        if n.waiting_incident {
            // Only an effect the node has already moved past may still be in flight.
            let live_effect = matches!(n.effect, Some(e) if !e.cancelling);
            if live_effect || n.ticket.is_some() || n.phase.holding() || n.phase.is_closed() {
                return Err(Broken::Incident);
            }
            if !n.phase.is_stopping() && (n.run.is_some() || n.job.is_some() || n.pool) {
                return Err(Broken::Incident);
            }
        }

        if n.credit.withheld
            && matches!(n.question, Some(q) if !matches!(q.cont,
                Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere))
        {
            return Err(Broken::Dependencies);
        }
        if n.credit.withheld
            && (!n.credit.ready
                || !matches!(
                    n.phase,
                    Phase::Ingesting
                        | Phase::Auditing
                        | Phase::Unplanned
                        | Phase::AwaitingRob
                        | Phase::Stopping(_)
                        | Phase::Closed(_)
                ))
        {
            return Err(Broken::Dependencies);
        }
        // Credit. It is set up when ingest finishes, so everything after has it.
        if n.credit.ledger_balance() < 0 {
            return Err(Broken::Credit);
        }
        if !n.credit.ready {
            let c = n.credit;
            if c.start != 0 || c.earned != 0 || c.added != 0 || c.spent != 0 || c.merge_credited {
                return Err(Broken::Credit);
            }
        }
        let leaf = n.phase.holding() || matches!(n.phase, Phase::Ready(_));
        let after_ingest = matches!(
            n.phase,
            Phase::Auditing | Phase::Unplanned | Phase::Planning { .. }
        );
        if (leaf || after_ingest || n.had_pr) && !n.credit.ready {
            return Err(Broken::Credit);
        }

        // What each phase needs.
        if let Phase::Ready(r) = n.phase {
            self.check_ready(n, r)?;
        }
        if let Some(r) = n.phase.resume() {
            self.check_ready(n, r)?;
        }
        if n.phase == Phase::Queued && !n.pr_open {
            return Err(Broken::Pr);
        }
        // The PR counts as open from the moment TL3 asks GitHub to open it.
        let opening = n.phase == Phase::Publishing
            && n.pos == 2
            && matches!(n.effect, Some(e) if !e.cancelling && e.kind == EffectKind::OpenPr);
        if n.phase == Phase::Publishing && (n.pos >= 3 || opening) && !n.pr_open {
            return Err(Broken::Pr);
        }
        if n.pr_open && !n.had_pr {
            return Err(Broken::Pr);
        }

        // Teardown goes in order, so what an earlier step removed stays gone.
        if n.phase.is_stopping() && ((n.pos > 2 && n.pr_open) || (n.pos > 4 && n.worktree)) {
            return Err(Broken::Teardown);
        }

        // Merges. A recorded merge means the outcome is Merged.
        if n.merged.is_some()
            && (!n.had_pr
                || n.pr_open
                || !matches!(
                    n.phase,
                    Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged)
                ))
        {
            return Err(Broken::Merge);
        }
        if matches!(
            n.phase,
            Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged)
        ) && n.merged.is_none()
        {
            return Err(Broken::Merge);
        }
        if n.phase == Phase::Closed(Outcome::Merged) && !n.credit.merge_credited {
            return Err(Broken::Merge);
        }

        // A closed node holds nothing.
        if n.phase.is_closed()
            && (n.effect.is_some()
                || n.run.is_some()
                || n.job.is_some()
                || n.pool
                || n.holds
                || n.ticket.is_some()
                || n.question.is_some()
                || n.waiting_incident
                || n.pr_open
                || n.worktree)
        {
            return Err(Broken::Closed);
        }
        Ok(())
    }

    }

    kernel_fn! {
    /// #771: verify the real leaf-readiness checker against its ghost predicate.
    fn check_ready(&self, n: &Node, r: Resume) -> (result: Result<(), Broken>) [
        ensures result.is_ok() == ready_checked(*n, r),
    ] {
        if n.attempt == 0
            || n.attempt > 3
            || n.round > 1
            || n.footprint.is_empty()
            || !n.credit.ready
        {
            return Err(Broken::Leaf);
        }
        let ok = match r {
            Resume::Making => true,
            Resume::Checking | Resume::Rebasing => n.head.is_some(),
            Resume::Reviewing => n.head.is_some() && n.checked == n.head,
            Resume::Publishing => match n.head {
                Some(h) => n.approved == Some(h) && n.checked == Some(h),
                None => false,
            },
        };
        if ok { Ok(()) } else { Err(Broken::Leaf) }
    }
    }
}
