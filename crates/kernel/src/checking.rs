//! Production consistency and settledness queries, with their safety contracts.
use super::*;

#[cfg(verus_keep_ghost)]
verus! {
/// K4a: the complete production checker as one named acceptance predicate.
/// `World::check` returns Ok exactly when it holds, for every finite world
/// allowed by the existing requires. No size, settledness or fixture bound.
/// It contains every rule of the real World::check: the complete dependency checker
/// (declarations, blocked-owner resources, unique keys and acyclicity), gate and
/// pool configuration, every node rule, disjoint held footprints per repository,
/// per-repository width, pool capacity and unique tickets.
#[verifier::opaque]
pub closed spec fn accepted(w: World) -> bool {
    dependencies::rules_ok(w) && progress_gate_apply::accepted(w)
}

/// The complete acceptance as its dependency and non-dependency parts.
pub(super) proof fn accepted_parts(w: World)
    ensures accepted(w) == (dependencies::rules_ok(w) && progress_gate_apply::accepted(w)),
{
    reveal(accepted);
}

/// Acceptance depends only on the world's views and scalar configuration.
pub(super) proof fn accepted_frame(before: World, after: World)
    requires accepted(before), after.nodes@ == before.nodes@,
        after.dependencies@ == before.dependencies@, after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity, after.next_ticket == before.next_ticket,
        after.next_question == before.next_question, after.file_sets@ == before.file_sets@,
    ensures accepted(after),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    dependencies::frame_preserves(before, after);
    assert forall|j: int| 0 <= j < after.nodes@.len()
        implies retirement::node_rules(after, #[trigger] after.nodes@[j]) by {
        retirement::node_frame(before, after, before.nodes@[j]);
    }
    assert forall|a: int, b: int| 0 <= a < b < after.nodes@.len()
        implies progress_gate_apply::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        assert(progress_gate_apply::pair(before, a, b));
    }
}

/// The complete acceptance, spelled out for proof consumers.
pub(super) proof fn accepted_definition(w: World)
    ensures accepted(w) == (dependencies::rules_ok(w)
        && w.gates@.len() > 0 && w.pool_capacity > 0
        && (forall|r: int| 0 <= r < w.gates@.len() ==> w.gates@[r].w > 0
            && progress_gate_apply::holders(w, RepoId(r as u64), w.nodes@.len()) <= w.gates@[r].w)
        && (forall|i: int| 0 <= i < w.nodes@.len() ==> retirement::node_rules(w, w.nodes@[i]))
        && (forall|a: int, b: int| 0 <= a < b < w.nodes@.len() ==> progress_gate_apply::pair(w, a, b))
        && pool_users(w.nodes@, w.nodes@.len()) <= w.pool_capacity
        && (forall|a: int, b: int| 0 <= a < w.nodes@.len() && 0 <= b < w.nodes@.len() && a != b
            ==> (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket))),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
}

/// Complete acceptance contains the older progress projection and graph indexing.
pub(super) proof fn accepted_checked(w: World)
    requires accepted(w),
    ensures world_checked(w), dependencies::rules_ok(w), dependencies::edges_indexable(w),
        dependencies::dependencies_checked(w),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    dependencies::rules_checked(w);
    world_checked_definition(w);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies node_checked(w, w.nodes@[j]) by {
        reveal(retirement::node_rules);
        assert(retirement::node_rules(w, w.nodes@[j]));
    }
}

/// One accepted node satisfies every check_node rule.
pub(super) proof fn accepted_node(w: World, i: int)
    requires accepted(w), 0 <= i < w.nodes@.len(),
    ensures retirement::node_rules(w, w.nodes@[i]), node_checked(w, w.nodes@[i]),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    reveal(retirement::node_rules);
}

/// Every issued place in line predates the next ticket and no two nodes share one.
/// This is check_node's `t < next_ticket` rule plus the checker's uniqueness scan.
pub(super) proof fn tickets_issued(w: World)
    requires accepted(w),
    ensures forall|i: int| 0 <= i < w.nodes@.len() ==> match #[trigger] w.nodes@[i].ticket {
            None => true, Some(t) => t < w.next_ticket,
        },
        forall|a: int, b: int| 0 <= a < w.nodes@.len() && 0 <= b < w.nodes@.len() && a != b
            && (#[trigger] w.nodes@[a]).ticket.is_some()
            ==> w.nodes@[a].ticket != (#[trigger] w.nodes@[b]).ticket,
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    assert forall|i: int| 0 <= i < w.nodes@.len() implies match #[trigger] w.nodes@[i].ticket {
        None => true, Some(t) => t < w.next_ticket,
    } by {
        reveal(retirement::node_rules);
        assert(retirement::node_rules(w, w.nodes@[i]));
    }
}
}

impl World {
    kernel_fn! {
    /// #771: true from the actual settle probe excludes every sufficient
    /// internal move used by the gate and pool obstruction argument.
    /// #806: zero remaining internal height also proves the real probe succeeds.
    pub fn is_settled(&self) -> (result: bool) [
        requires settle_ready(*self),
        ensures result ==> settled_world(*self),
            progress_phase::zero_internal_height(*self) ==> result,
    ] proof [reveal(settled_world); progress_phase::zero_internal_height_definition(*self);] {
        let mut w = snapshot_world(self);
        let mut out = Outbox::new();
        kernel_proof! { reveal(empty_outbox); }
        let mut i = 0usize;
        kernel_while! { (i < w.nodes.len()) [
            invariant i <= w.nodes.len(), settle_ready(w), out.len == 0,
                w.nodes@ == self.nodes@, w.gates@ == self.gates@,
                w.pool_capacity == self.pool_capacity, w.dependencies@ == self.dependencies@,
                w.next_ticket == self.next_ticket,
                progress_phase::zero_internal_height(*self) ==>
                    potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) == 0,
                forall|j: int| 0 <= j < i ==> !movable(*self, j),
            decreases w.nodes.len() - i,
        ] {
            kernel_proof! {
                potential_member(w.nodes@, i as int, w.nodes@.len(), |n: Node| ticket_capacity(n));
            }
            kernel_snapshot! { before_probe = w }
            if advance(&mut w, i, &mut out, &mut Report::new()) { return false; }
            kernel_proof! {
                movable_frame(*self, before_probe@, i as int);
                reveal(advance_post); reveal(settle_ready); reveal(scheduling_safe);
            }
            i += 1;
        } }
        true
    }
    }

    kernel_fn! {
    /// #771: successful executable consistency checking supplies the exact
    /// progress invariants used by the source-connected theorem.
    pub fn consistent(&self) -> (result: bool) [
        requires world_headroom(*self) || dependency_retirement_domain(*self), storage_ordered(*self),
        ensures result ==> world_checked(*self),
            dependency_retirement_domain(*self) ==> result == dependency_retirement_accepted(*self),
            result == accepted(*self),
    ] proof [reveal(storage_ordered); reveal(dependency_retirement_domain); reveal(dependency_retirement_accepted);] {
        self.check().is_ok()
    }
    }

    kernel_fn! {
    /// #771: the real world checker establishes positive capacities and the
    /// per-node and prerequisite facts, without changing any acceptance rule.
    pub fn check(&self) -> (result: Result<(), Broken>) [
        requires world_headroom(*self) || dependency_retirement_domain(*self), storage_ordered(*self),
        ensures result.is_ok() ==> world_checked(*self),
            dependency_retirement_domain(*self) ==> result.is_ok() == dependency_retirement_accepted(*self),
            retirement_domain(*self) ==> result.is_ok() == retirement_accepted(*self),
            progress_gate_check_result(*self, result.is_ok()),
            gate_apply_checker_domain(*self) ==> result.is_ok() == gate_apply_checker_accepted(*self),
            unplanned_check_domain(*self)
                ==> result.is_ok() == retirement_accepted(*self),
            result.is_ok() == accepted(*self),
    ] proof [reveal(accepted); reveal(world_headroom); reveal(world_checked); reveal(storage_ordered);
        reveal(dependency_retirement_domain); reveal(dependency_retirement_accepted);
        reveal_with_fuel(open_prefix, 2); reveal_with_fuel(potential_prefix, 2);
        reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked);
        reveal(unplanned_check_domain); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
        reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5);] {
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
            kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
            if self.gates[gate_index].w == 0 { return Err(Broken::Config); }
            gate_index += 1;
        } }
        let mut i = 0;
        kernel_while! { (i < self.nodes.len()) [
            invariant i <= self.nodes.len(),
                forall|j: int| 0 <= j < i ==> node_checked(*self, self.nodes@[j]),
                forall|j: int| 0 <= j < i ==> retirement::node_rules(*self, self.nodes@[j]),
            decreases self.nodes.len() - i,
        ] {
            kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
            self.check_node(&self.nodes[i])?;
            i += 1;
        } }
        // Locks are the footprints of holders in the same repo.
        let mut i = 0;
        kernel_while! { (i < self.nodes.len()) [
            invariant i <= self.nodes.len(), footprint::ordered_table(*self),
                progress_gate::domain(*self) ==> forall|a: int, b: int| 0 <= a < b < i
                    && self.nodes@[a].holds && self.nodes@[b].holds && self.nodes@[a].repo == self.nodes@[b].repo
                    ==> footprint::disjoint(*self, self.nodes@[a].footprint, self.nodes@[b].footprint),
                forall|a: int, b: int| 0 <= a < b < i ==> progress_gate_apply::pair(*self, a, b),
                forall|j: int| 0 <= j < self.nodes@.len() ==> retirement::node_rules(*self, self.nodes@[j]),
                (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) && i >= 2 ==>
                    (self.nodes@[0].holds && self.nodes@[1].holds && self.nodes@[0].repo == self.nodes@[1].repo
                        ==> footprint::disjoint(*self, self.nodes@[0].footprint, self.nodes@[1].footprint)),
                forall|j: int| 0 <= j < self.nodes@.len() ==> node_checked(*self, self.nodes@[j]),
            decreases self.nodes.len() - i,
        ] {
            kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
            let n = self.nodes[i];
            if n.holds {
                let mut j = 0;
                kernel_while! { (j < i) [
                    invariant j <= i < self.nodes.len(), n == self.nodes@[i as int], n.holds,
                        progress_gate::domain(*self) ==> forall|k: int| 0 <= k < j
                            && self.nodes@[k].holds && self.nodes@[k].repo == n.repo
                            ==> footprint::disjoint(*self, self.nodes@[k].footprint, n.footprint),
                        forall|k: int| 0 <= k < j ==> progress_gate_apply::pair(*self, k, i as int),
                        (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) && i == 1 && j >= 1 && n.holds
                            && self.nodes@[0].holds && self.nodes@[0].repo == n.repo
                            ==> footprint::disjoint(*self, self.nodes@[0].footprint, n.footprint),
                        footprint::ordered_table(*self),
                        forall|k: int| 0 <= k < self.nodes@.len() ==> node_checked(*self, self.nodes@[k]),
                    decreases i - j,
                ] {
                    kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                        reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
                    let other = self.nodes[j];
                    kernel_proof! {
                        assert(progress_gate_apply::accepted(*self) ==> progress_gate_apply::pair(*self, j as int, i as int));
                        assert(node_checked(*self, n));
                        assert(node_checked(*self, other));
                    }
                    if other.holds
                        && other.repo == n.repo
                        && !self.footprints_disjoint(other.footprint, n.footprint)
                    {
                        kernel_proof! {
                            if progress_gate::domain(*self) && progress_gate::accepted(*self) {
                                assert(progress_gate::pair(*self, j as int, i as int));
                            }
                        }
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
                progress_gate::domain(*self) && r > 0 ==> progress_gate::holders(*self, 4) <= self.gates@[0].w,
                forall|k: int| 0 <= k < r ==> progress_gate_apply::holders(*self, RepoId(k as u64), self.nodes@.len()) <= self.gates@[k].w,
                (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) ==> forall|k: int| 0 <= k < r
                    ==> retirement::holders(*self, RepoId(k as u64), 2) <= self.gates@[k].w,
            decreases self.gates.len() - r,
        ] {
            kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
            let mut count = 0usize;
            let mut index = 0usize;
            kernel_while! { (index < self.nodes.len()) [
                invariant index <= self.nodes.len(), count <= index,
                    progress_gate::domain(*self) && r == 0 ==> count == progress_gate::holders(*self, index as nat),
                    count == progress_gate_apply::holders(*self, RepoId(r as u64), index as nat),
                    (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) ==> count == retirement::holders(*self, RepoId(r as u64), index as nat),
                decreases self.nodes.len() - index,
            ] {
                kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                    reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal(potential_prefix); reveal_with_fuel(potential_prefix, 5); }
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
                progress_gate::domain(*self) ==> runs == progress_gate::runs(*self, index as nat),
                runs == pool_users(self.nodes@, index as nat),
                (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) ==> runs as int ==
                    (if index > 0 && self.nodes@[0].pool { 1int } else { 0int })
                    + (if index > 1 && self.nodes@[1].pool { 1int } else { 0int }),
                open_prefix(self.nodes@, self.nodes@.len()) <= u32::MAX as int,
                forall|j: int| 0 <= j < self.nodes@.len() ==> node_checked(*self, self.nodes@[j]),
            decreases self.nodes.len() - index,
        ] {
            kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
            kernel_proof! {
                reveal(open_prefix); reveal(potential_prefix);
                open_prefix_monotone(self.nodes@, index as nat + 1, self.nodes@.len());
            }
            if self.nodes[index].pool { runs += 1; }
            index += 1;
        } }
        if runs > self.pool_capacity as u32 { return Err(Broken::Pool); }
        let mut a = 0usize;
        kernel_while! { (a < self.nodes.len()) [
            invariant a <= self.nodes.len(),
                progress_gate::domain(*self) ==> forall|x: int, y: int| 0 <= x < a && x < y < 4
                    && self.nodes@[x].ticket.is_some() ==> self.nodes@[x].ticket != self.nodes@[y].ticket,
                forall|j: int, k: int| 0 <= j < a && 0 <= k < self.nodes@.len() && j != k
                    ==> (self.nodes@[j].ticket.is_some() ==> self.nodes@[j].ticket != self.nodes@[k].ticket),
                (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) && a > 0 ==>
                    (self.nodes@[0].ticket.is_some() ==> self.nodes@[0].ticket != self.nodes@[1].ticket),
            decreases self.nodes.len() - a,
        ] {
            kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
            let x = &self.nodes[a];
            let mut b = 0usize;
            kernel_while! { (b < self.nodes.len()) [
                invariant a < self.nodes.len(), b <= self.nodes.len(), *x == self.nodes@[a as int],
                    progress_gate::domain(*self) ==> forall|k: int| a < k < b
                        && x.ticket.is_some() ==> x.ticket != self.nodes@[k].ticket,
                    forall|k: int| 0 <= k < b && a != k
                        ==> (x.ticket.is_some() ==> x.ticket != self.nodes@[k].ticket),
                    (retirement::domain(*self) || progress_unplanned_witness::check_domain(*self)) && a == 0 && b > 1 ==>
                        (self.nodes@[0].ticket.is_some() ==> self.nodes@[0].ticket != self.nodes@[1].ticket),
                decreases self.nodes.len() - b,
            ] {
                kernel_proof! { reveal(accepted); reveal(progress_gate_check_result); reveal(progress_gate_contracts::checked); reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::accepted);
                    reveal(progress_gate::accepted); reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal_with_fuel(potential_prefix, 5); }
                let y = &self.nodes[b];
                if a != b && x.ticket.is_some() && x.ticket == y.ticket {
                    kernel_proof! {
                        if progress_gate::domain(*self) && progress_gate::accepted(*self) {
                            if a < b { assert(progress_gate::pair(*self, a as int, b as int)); }
                            else { assert(progress_gate::pair(*self, b as int, a as int)); }
                        }
                    }
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
            result.is_ok() == retirement::node_rules(*self, *n),
    ] proof [reveal(retirement::node_rules);] {
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
                && !matches!(n.phase, Phase::Stopping(_)))
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
        if n.phase == Phase::AwaitingDelivery {
            self.check_ready(n, Resume::Publishing)?;
            if !n.pr_open {
                return Err(Broken::Pr);
            }
            if n.effect.is_some() {
                return Err(Broken::Effect);
            }
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
