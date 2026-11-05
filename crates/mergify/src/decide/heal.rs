//! Body-heal decisions over parsed upstream inputs; no clocks or I/O.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! decision_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! decision_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub same_head: bool,
    pub reran: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Publishing,
    Queued,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    None,
    Saved,
    Issue,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rerun<E> {
    Continue,
    KeepPending(E),
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub id: u64,
    pub current_head: bool,
    pub pull_request: bool,
    pub listed: bool,
    pub completed: bool,
    pub failed: bool,
}

/// Preserved existing behaviour (no rule): select the first matching repository policy; later duplicates do not override it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match out { Some(i) => i < matches@.len() && matches@[i as int] && forall|j: int| 0 <= j < i ==> !matches@[j], None => forall|j: int| 0 <= j < matches@.len() ==> !matches@[j] },))]
pub fn policy(matches: &[bool]) -> Option<usize> {
    let mut i = 0;
    decision_while! { (i < matches.len()) [
        invariant i <= matches@.len(), forall|j: int| 0 <= j < i ==> !matches@[j],
        decreases matches@.len() - i,
    ] { if matches[i] { return Some(i); } i += 1; } }
    None
}

/// Preserved existing behaviour (no rule): healing requires an open, unmerged PR.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (open && !merged),))]
pub fn available(open: bool, merged: bool) -> bool {
    open && !merged
}

/// Preserved existing behaviour (no rule): only a configured branch owner can heal; absent branch configuration skips healing.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == matches!(owner, Some(true)),))]
pub fn owned(owner: Option<bool>) -> bool {
    matches!(owner, Some(true))
}

/// Preserved existing behaviour (no rule): only an unfinished heal for the same head is pending.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == matches!(record, Some(r) if r.same_head && !r.reran),))]
pub fn pending(record: Option<Record>) -> bool {
    matches!(record, Some(r) if r.same_head && !r.reran)
}

/// Preserved existing behaviour (no rule): phase is sampled only for gaps or a pending heal.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!gaps_empty || pending),))]
pub fn needs_phase(gaps_empty: bool, pending: bool) -> bool {
    !gaps_empty || pending
}

/// Preserved existing behaviour (no rule): only Publishing/Queued permit a heal.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !matches!(phase, Phase::Other),))]
pub fn phase(phase: Phase) -> bool {
    !matches!(phase, Phase::Other)
}

/// Preserved existing behaviour (no rule): no gaps skip issue reads; saved request text skips the upstream issue read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == if gaps_empty { Request::None } else if saved { Request::Saved } else { Request::Issue },))]
pub fn request(gaps_empty: bool, saved: bool) -> Request {
    if gaps_empty {
        Request::None
    } else if saved {
        Request::Saved
    } else {
        Request::Issue
    }
}

/// Preserved existing behaviour (no rule): an empty rerun workflow policy must skip workflow_runs.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !empty,))]
pub fn read_runs(empty: bool) -> bool {
    !empty
}

/// Preserved existing behaviour (no rule): a failed rerun returns before the completed-heal write and leaves the saved heal pending.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match result { Ok(()) => matches!(out, Rerun::Continue), Err(error) => matches!(out, Rerun::KeepPending(e) if equal(e, error)) },))]
pub fn rerun_result<E>(result: Result<(), E>) -> Rerun<E> {
    match result {
        Ok(()) => Rerun::Continue,
        Err(error) => Rerun::KeepPending(error),
    }
}

/// Preserved existing behaviour (no rule): only current-head pull_request runs of a listed workflow enter newest-per-path selection.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (run.current_head && run.pull_request && run.listed),))]
pub fn eligible(run: &Run) -> bool {
    run.current_head && run.pull_request && run.listed
}

#[cfg(verus_keep_ghost)]
vstd::prelude::verus! {
    pub open spec fn candidate(run: Run) -> bool { run.current_head && run.pull_request && run.listed }
}

/// Preserved existing behaviour (no rule): choose maximum eligible id within one exact path group, keeping the first run on a tie. Status is checked after selection.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match out {
    None => forall|j: int| 0 <= j < runs@.len() ==> !candidate(runs@[j]),
    Some(i) => i < runs@.len() && candidate(runs@[i as int]) && forall|j: int| 0 <= j < runs@.len() && candidate(runs@[j]) ==> (runs@[j].id < runs@[i as int].id || (runs@[j].id == runs@[i as int].id && i <= j)),
},))]
pub fn newest(runs: &[Run]) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut i = 0;
    decision_while! { (i < runs.len()) [
        invariant i <= runs@.len(),
            match best {
                None => forall|j: int| 0 <= j < i ==> !candidate(runs@[j]),
                Some(b) => b < i && candidate(runs@[b as int]) && forall|j: int| 0 <= j < i && candidate(runs@[j]) ==> (runs@[j].id < runs@[b as int].id || (runs@[j].id == runs@[b as int].id && b <= j)),
            },
        decreases runs@.len() - i,
    ] {
        if eligible(&runs[i]) {
            best = match best { Some(previous) => if runs[i].id > runs[previous].id { Some(i) } else { best }, None => Some(i) };
        }
        i += 1;
    } }
    best
}

/// Preserved existing behaviour (no rule): rerun only completed runs with failure/cancelled/timed_out/startup_failure conclusions.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (run.completed && run.failed),))]
pub fn failed(run: &Run) -> bool {
    run.completed && run.failed
}
