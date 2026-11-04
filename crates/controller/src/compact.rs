//! #783 / Q27: compact observations of real steps, with periodic full baselines.
use serde::{Deserialize, Serialize};
use tl3_kernel::{Dependency, Event, FileId, Gate, Node, World};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rows<T> {
    pub len: usize,
    pub puts: Vec<(usize, T)>,
}
impl<T: Clone + PartialEq> Rows<T> {
    fn between(before: &[T], after: &[T]) -> Self {
        Self {
            len: after.len(),
            puts: after
                .iter()
                .enumerate()
                .filter(|(i, v)| before.get(*i) != Some(*v))
                .map(|(i, v)| (i, v.clone()))
                .collect(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    pub nodes: Rows<Node>,
    pub gates: Rows<Gate>,
    pub file_sets: Rows<Vec<FileId>>,
    pub dependencies: Rows<Dependency>,
    pub pool_capacity: u8,
    pub next_ticket: u64,
    pub next_question: u64,
    pub fail_fast: bool,
}
impl Delta {
    pub fn between(before: &World, after: &World) -> Self {
        Self {
            nodes: Rows::between(&before.nodes, &after.nodes),
            gates: Rows::between(&before.gates, &after.gates),
            file_sets: Rows::between(before.file_sets(), after.file_sets()),
            dependencies: Rows::between(before.dependencies(), after.dependencies()),
            pool_capacity: after.pool_capacity,
            next_ticket: after.next_ticket,
            next_question: after.next_question,
            fail_fast: after.fail_fast,
        }
    }
    /// Offline only: reconstruct private kernel fields through its authoritative serde type.
    /// Reject duplicate indexes, gaps and truncations rather than hiding malformed deltas.
    pub fn apply(&self, world: &mut World) -> Result<(), String> {
        fn patch<T: Serialize>(
            value: &mut serde_json::Value,
            name: &str,
            rows: &Rows<T>,
        ) -> Result<(), String> {
            let array = value[name]
                .as_array_mut()
                .ok_or("delta field is not an array")?;
            array.resize(rows.len, serde_json::Value::Null);
            let mut seen = std::collections::BTreeSet::new();
            for (i, item) in &rows.puts {
                if *i >= rows.len || !seen.insert(*i) {
                    return Err("invalid delta index".into());
                }
                array[*i] = serde_json::to_value(item).map_err(|_| "invalid delta value")?;
            }
            if array.iter().any(serde_json::Value::is_null) {
                return Err("gap in delta rows".into());
            }
            Ok(())
        }
        let mut value = serde_json::to_value(&*world).map_err(|_| "cannot encode delta input")?;
        patch(&mut value, "nodes", &self.nodes)?;
        patch(&mut value, "gates", &self.gates)?;
        patch(&mut value, "file_sets", &self.file_sets)?;
        patch(&mut value, "dependencies", &self.dependencies)?;
        value["pool_capacity"] = self.pool_capacity.into();
        value["next_ticket"] = self.next_ticket.into();
        value["next_question"] = self.next_question.into();
        value["fail_fast"] = self.fail_fast.into();
        *world = serde_json::from_value(value).map_err(|_| "invalid reconstructed delta world")?;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompactStep {
    pub preparation: Delta,
    pub event: Event,
    pub after: Delta,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub format: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<World>,
    pub baseline_revision: i64,
    pub previous_kernel_revision: Option<i64>,
    pub event_count: usize,
    pub steps: Vec<CompactStep>,
    pub settle: Option<Delta>,
    pub final_delta: Delta,
}
/// #783: replay every real step, and require its result to match the recorded delta.
pub fn check(record: &Record, previous: Option<&World>, revision: i64) -> Result<World, String> {
    let fail = |reason: &str| {
        format!(
            "unverifiable from revision {revision}: {reason}; baseline revision {}",
            record.baseline_revision
        )
    };
    if record.format != 1 || record.event_count != record.steps.len() {
        return Err(fail("incomplete delta record"));
    }
    let mut world = match (&record.baseline, previous) {
        (Some(baseline), Some(previous)) if baseline != previous => {
            return Err(fail("baseline chain diverged"));
        }
        (Some(baseline), _) => baseline.clone(),
        (None, Some(previous)) => previous.clone(),
        (None, None) => return Err(fail("stored baseline required")),
    };
    crate::verification::check_world(&world)
        .map_err(|e| fail(&format!("baseline invariant {e}")))?;
    for step in &record.steps {
        let mut input = world.clone();
        step.preparation.apply(&mut input).map_err(|e| fail(&e))?;
        // Translation may only append footprint sets before an event.
        let mut allowed = serde_json::to_value(&world).map_err(|_| fail("invalid input"))?;
        allowed["file_sets"] =
            serde_json::to_value(input.file_sets()).map_err(|_| fail("invalid footprint sets"))?;
        if !input.file_sets().starts_with(world.file_sets())
            || allowed != serde_json::to_value(&input).map_err(|_| fail("invalid preparation"))?
        {
            return Err(fail("preparation changed request state"));
        }
        let actual = tl3_kernel::step(input.clone(), step.event).0;
        crate::verification::check_world(&actual).map_err(|e| fail(&format!("invariant {e}")))?;
        let mut recorded = input;
        step.after.apply(&mut recorded).map_err(|e| fail(&e))?;
        if actual != recorded {
            return Err(fail("step delta diverged"));
        }
        world = actual;
    }
    let mut recorded = world.clone();
    record
        .final_delta
        .apply(&mut recorded)
        .map_err(|e| fail(&e))?;
    if let Some(settle) = &record.settle {
        if !record.steps.is_empty() {
            return Err(fail("settle mixed with event steps"));
        }
        let mut input = world.clone();
        settle.apply(&mut input).map_err(|e| fail(&e))?;
        if input.nodes != world.nodes
            || input.file_sets() != world.file_sets()
            || input.dependencies() != world.dependencies()
            || input.next_ticket != world.next_ticket
            || input.next_question != world.next_question
        {
            return Err(fail("capacity preparation changed request state"));
        }
        world = tl3_kernel::settle_world(input).0;
        crate::verification::check_world(&world).map_err(|e| fail(&format!("invariant {e}")))?;
    }
    if world != recorded {
        return Err(fail("final delta diverged"));
    }
    Ok(world)
}
