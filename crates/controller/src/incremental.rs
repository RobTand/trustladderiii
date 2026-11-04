//! #783 / Q27: cached observations of existing invariants, never admission rules.
use std::collections::{BTreeMap, BTreeSet};
use tl3_kernel::{Node, World};

/// Bound recovery to 127 intervening kernel commits while amortizing full baselines.
pub const BASELINE_INTERVAL: u64 = 128;
// RED mutant: deliberately leave pool usage stale after the first observation.
const UPDATE_POOL_SUMMARY: bool = false;

#[derive(Default)]
pub struct Incremental {
    previous: Option<World>,
    node_failures: BTreeMap<usize, String>,
    holders: BTreeSet<usize>,
    holder_counts: BTreeMap<u64, usize>,
    tickets: BTreeMap<u64, usize>,
    duplicate_tickets: usize,
    pool: usize,
    dependency_error: Option<String>,
    steps: u64,
    pub full_checks: u64,
    pub touched_nodes: usize,
}
fn add_count(counts: &mut BTreeMap<u64, usize>, key: u64, add: bool) -> (usize, usize) {
    let old = counts.get(&key).copied().unwrap_or(0);
    let new = if add { old + 1 } else { old - 1 };
    if new == 0 {
        counts.remove(&key);
    } else {
        counts.insert(key, new);
    }
    (old, new)
}
impl Incremental {
    fn summary(&mut self, index: usize, node: &Node, add: bool, update_pool: bool) {
        if node.pool && update_pool {
            if add {
                self.pool += 1;
            } else {
                self.pool -= 1;
            }
        }
        if node.holds {
            if add {
                self.holders.insert(index);
            } else {
                self.holders.remove(&index);
            }
            add_count(&mut self.holder_counts, node.repo.0, add);
        }
        if let Some(ticket) = node.ticket {
            let (old, new) = add_count(&mut self.tickets, ticket, add);
            if old < 2 && new >= 2 {
                self.duplicate_tickets += 1;
            }
            if old >= 2 && new < 2 {
                self.duplicate_tickets -= 1;
            }
        }
    }
    /// #783: compare actual typed outputs, including non-triggering settled nodes.
    /// Cached dependency results are invalidated by every endpoint/withheld change.
    pub fn check(&mut self, world: &World) -> Result<(), String> {
        let previous = self.previous.take();
        let old = previous.as_ref();
        let mut touched = Vec::new();
        for index in 0..world.nodes.len().max(old.map_or(0, |w| w.nodes.len())) {
            let before = old.and_then(|w| w.nodes.get(index));
            let after = world.nodes.get(index);
            if before != after {
                let update_pool = old.is_none() || UPDATE_POOL_SUMMARY;
                if let Some(n) = before {
                    self.summary(index, n, false, update_pool);
                }
                if let Some(n) = after {
                    self.summary(index, n, true, update_pool);
                }
                if after.is_some() {
                    touched.push(index);
                }
                self.node_failures.remove(&index);
            }
        }
        self.touched_nodes = touched.len();
        let deps_changed = old.is_none_or(|w| {
            w.dependencies() != world.dependencies() || w.nodes.len() != world.nodes.len()
        });
        let dependency_node_changed = touched.iter().any(|&i| {
            world.nodes[i].credit.withheld
                || old.is_some_and(|w| w.nodes.get(i).is_some_and(|n| n.credit.withheld))
                || world.dependencies().iter().any(|d| {
                    d.node.0 == i as u64 || d.target.request.is_some_and(|n| n.0 == i as u64)
                })
        });
        if deps_changed || dependency_node_changed {
            self.dependency_error = world.check_dependencies().err().map(|b| format!("{b:?}"));
        }
        let all_local = old.is_none_or(|w| {
            w.gates.len() != world.gates.len() || w.file_sets() != world.file_sets()
        });
        let thresholds = old.is_some_and(|w| {
            w.next_ticket != world.next_ticket || w.next_question != world.next_question
        });
        if all_local {
            touched = (0..world.nodes.len()).collect();
        } else if thresholds {
            touched.extend(
                world
                    .nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| (n.ticket.is_some() || n.question.is_some()).then_some(i)),
            );
            touched.extend(self.node_failures.keys().copied());
            touched.sort_unstable();
            touched.dedup();
        }
        for i in touched {
            if let Err(b) = world.check_node(&world.nodes[i]) {
                self.node_failures.insert(i, format!("{b:?}"));
            } else {
                self.node_failures.remove(&i);
            }
        }
        let mut result = self.consistent(world);
        if result.is_ok() {
            // An unchanged waiter can become runnable after another node releases
            // resources. Include every possible waiter, not only changed nodes.
            let candidates: Vec<_> = world
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(i, n)| (!n.phase.is_closed() && n.effect.is_none()).then_some(i))
                .collect();
            if !world.is_settled_at(&candidates) {
                result = Err("Unsettled".into());
            }
        }
        self.steps += 1;
        if self.steps == 1 || self.steps.is_multiple_of(BASELINE_INTERVAL) {
            self.full_checks += 1;
            let full = crate::verification::check_world(world);
            if full != result {
                result = Err(format!(
                    "IncrementalDrift: cached={result:?}, full={full:?}"
                ));
            }
        }
        self.previous = Some(world.clone());
        result
    }
    fn consistent(&self, world: &World) -> Result<(), String> {
        if let Some(error) = &self.dependency_error {
            return Err(error.clone());
        }
        if world.gates.is_empty()
            || world.pool_capacity == 0
            || world.gates.iter().any(|g| g.w == 0)
        {
            return Err("Config".into());
        }
        if let Some((_, error)) = self.node_failures.first_key_value() {
            return Err(error.clone());
        }
        for &i in &self.holders {
            let n = &world.nodes[i];
            for &j in self.holders.range(..i) {
                let other = &world.nodes[j];
                if n.repo == other.repo && !world.footprints_disjoint(n.footprint, other.footprint)
                {
                    return Err("Gate".into());
                }
            }
        }
        if world
            .gates
            .iter()
            .enumerate()
            .any(|(r, g)| self.holder_counts.get(&(r as u64)).copied().unwrap_or(0) > g.w as usize)
        {
            return Err("Gate".into());
        }
        if self.pool > world.pool_capacity as usize {
            return Err("Pool".into());
        }
        if self.duplicate_tickets > 0 {
            return Err("Ticket".into());
        }
        Ok(())
    }
}
