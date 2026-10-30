//! TL3 kernel trial. The kernel is pure: a step takes the item and one event and returns the
//! next item and at most one effect. Ordinary builds have no standard library, so the kernel
//! cannot reach files, sockets, clocks or processes. Kani builds and tests add std back.
#![cfg_attr(not(any(kani, test)), no_std)]

pub type Sha = u64;
pub type Attempt = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Role {
    Implementer,
    Reviewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Outcome {
    Pass,
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Verdict {
    Approve,
    Changes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Phase {
    Queued,
    Implementing,
    Testing { head: Sha },
    Reviewing { head: Sha },
    Merging { head: Sha },
    Closed { head: Sha },
    /// Out of attempts: the attempt counter cannot go higher, so the item waits for Rob.
    Held,
}

/// `tested` and `approved` name the commit that passed tests and the commit a reviewer approved.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Item {
    pub phase: Phase,
    pub attempt: Attempt,
    pub tested: Option<Sha>,
    pub approved: Option<Sha>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Event {
    Start,
    AgentDone { attempt: Attempt, head: Sha },
    TestResult { attempt: Attempt, head: Sha, outcome: Outcome },
    TestQuiet { attempt: Attempt },
    TestLeaseLost { attempt: Attempt },
    Reviewed { attempt: Attempt, head: Sha, verdict: Verdict },
    MergeDone { head: Sha },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    RunAgent { role: Role, attempt: Attempt },
    SubmitTests { attempt: Attempt, head: Sha },
    CheckTestRecord { attempt: Attempt },
    Merge { head: Sha },
    CloseIssue { head: Sha },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Out {
    pub item: Item,
    pub effect: Option<Effect>,
}

pub const INIT: Item = Item { phase: Phase::Queued, attempt: 0, tested: None, approved: None };

/// What an item needs in flight in its current phase. Every effect names its attempt or commit,
/// so asserting it a second time is harmless.
pub fn pending(item: &Item) -> Option<Effect> {
    let attempt = item.attempt;
    match item.phase {
        Phase::Queued | Phase::Held => None,
        Phase::Implementing => Some(Effect::RunAgent { role: Role::Implementer, attempt }),
        Phase::Testing { head } => Some(Effect::SubmitTests { attempt, head }),
        Phase::Reviewing { .. } => Some(Effect::RunAgent { role: Role::Reviewer, attempt }),
        Phase::Merging { head } => Some(Effect::Merge { head }),
        Phase::Closed { head } => Some(Effect::CloseIssue { head }),
    }
}

fn enter(item: Item) -> Out {
    let effect = pending(&item);
    Out { item, effect }
}

fn stay(item: Item) -> Out {
    Out { item, effect: None }
}

fn retry(item: &Item) -> Out {
    match item.attempt.checked_add(1) {
        Some(attempt) => enter(Item { phase: Phase::Implementing, attempt, tested: None, approved: None }),
        None => enter(Item { phase: Phase::Held, attempt: item.attempt, tested: None, approved: None }),
    }
}

/// Takes the item by value: once a step consumes it, the caller cannot keep using the old one.
pub fn step(item: Item, event: Event) -> Out {
    let current = item.attempt;
    match (item.phase, event) {
        (Phase::Queued, Event::Start) => {
            enter(Item { phase: Phase::Implementing, attempt: current, tested: None, approved: None })
        }
        (Phase::Implementing, Event::AgentDone { attempt, head }) if attempt == current => {
            enter(Item { phase: Phase::Testing { head }, attempt, tested: None, approved: None })
        }
        (Phase::Testing { head }, Event::TestResult { attempt, head: h, outcome })
            if attempt == current && h == head =>
        {
            match outcome {
                Outcome::Pass => enter(Item { phase: Phase::Reviewing { head }, attempt, tested: Some(head), approved: None }),
                Outcome::Fail => retry(&item),
            }
        }
        (Phase::Testing { .. }, Event::TestQuiet { attempt }) if attempt == current => {
            Out { item, effect: Some(Effect::CheckTestRecord { attempt }) }
        }
        (Phase::Reviewing { head }, Event::Reviewed { attempt, head: h, verdict })
            if attempt == current && h == head =>
        {
            match verdict {
                Verdict::Approve => enter(Item { phase: Phase::Merging { head }, approved: Some(head), ..item }),
                Verdict::Changes => retry(&item),
            }
        }
        (Phase::Merging { head }, Event::MergeDone { head: h }) if h == head => {
            enter(Item { phase: Phase::Closed { head }, ..item })
        }
        // Everything else leaves the item as it is: a lost lease, which PrismaBuild retries under
        // the same key; stale or duplicate events; anything after Closed or Held.
        _ => stay(item),
    }
}

/// Replays a log of events from an item. The shell's whole recovery is this fold.
pub fn run(item: Item, events: &[Event]) -> Item {
    events.iter().fold(item, |item, &event| step(item, event).item)
}

#[cfg(kani)]
mod laws {
    use super::*;

    fn safe(item: &Item) -> bool {
        match item.phase {
            Phase::Reviewing { head } => item.tested == Some(head),
            Phase::Merging { head } | Phase::Closed { head } => {
                item.tested == Some(head) && item.approved == Some(head)
            }
            _ => true,
        }
    }

    /// Law 1, induction step: no event takes a safe item to an unsafe one. With the base case
    /// below, an item merges or closes only on a commit that passed tests and that a reviewer
    /// approved, whatever events arrive, in any order, duplicated or stale.
    #[kani::proof]
    fn law1_no_event_breaks_merge_safety() {
        let item: Item = kani::any();
        kani::assume(safe(&item));
        let out = step(item, kani::any());
        assert!(safe(&out.item));
    }

    /// Law 1, base case, plus a direct check of every possible run of five events.
    #[kani::proof]
    #[kani::unwind(6)]
    fn law1_holds_for_every_five_event_run() {
        assert!(safe(&INIT));
        let events: [Event; 5] = kani::any();
        assert!(safe(&run(INIT, &events)));
    }

    /// Law 2: while tests run, only PrismaBuild's own result for this attempt and commit moves
    /// the item, never silence, a lost lease or a stale result. That result always moves it.
    #[kani::proof]
    fn law2_only_evidence_ends_a_wait() {
        let item: Item = kani::any();
        let Phase::Testing { head } = item.phase else { return };
        let current = item.attempt;
        let event: Event = kani::any();
        let awaited = matches!(event, Event::TestResult { attempt, head: h, .. } if attempt == current && h == head);
        let out = step(item.clone(), event);
        if out.item != item {
            assert!(awaited);
        }
        if awaited {
            assert!(out.item.phase != item.phase);
        }
    }

    /// Law 3: an event that moved an item is used up. Delivering it again changes nothing and
    /// starts nothing.
    #[kani::proof]
    fn law3_duplicate_is_harmless() {
        let item: Item = kani::any();
        let event: Event = kani::any();
        let first = step(item.clone(), event);
        if first.item != item {
            let again = step(first.item.clone(), event);
            assert!(again.item == first.item && again.effect.is_none());
        }
    }

    /// Law 4: a move emits exactly what the new state needs in flight, so after a crash the
    /// shell replays the log and asserts `pending` again: the same effect, under the same key.
    #[kani::proof]
    fn law4_recovery_equals_live_run() {
        let item: Item = kani::any();
        let out = step(item.clone(), kani::any());
        if out.item != item {
            assert!(out.effect == pending(&out.item));
        }
    }
}
