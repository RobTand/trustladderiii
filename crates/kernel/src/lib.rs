//! TL3's slice-one kernel, built from `docs/state-table.md` version 5.
//!
//! The kernel is one pure function, `step(world, event) -> (world, outputs)`.
//! After applying the event, `settle` takes every internal transition the
//! facts allow until none is left. Ordinary builds have no standard library,
//! so the kernel can't reach files, sockets, clocks or processes.
//!
//! Slice-one simplifications:
//! - Each request is a single root leaf, so its credit lives on its node.
//! - Rob owns every pause, as in slice one.
//! - Notices are outbox entries that no node waits on: questions to Rob,
//!   notes, issue summaries and filing a replacement request.
//! - Looks run in the controller. What they find arrives as replies,
//!   acknowledgements and facts.
//! - After a permanent effect failure, carrying on restarts the phase's
//!   effects from the top. Every effect converges or is deduplicated, so this
//!   ends where retrying only the failed effect would.
#![cfg_attr(not(any(kani, test)), no_std)]

extern crate alloc;

use alloc::vec::Vec;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[macro_use]
mod verification;

#[cfg(verus_keep_ghost)]
pub mod ranks;
#[cfg(not(verus_keep_ghost))]
mod ranks;
use ranks::advance_node;
#[cfg(verus_keep_ghost)]
use ranks::*;

mod dependencies;
pub use dependencies::Dependency;
mod footprint;
include!("headroom.rs");
mod ids;
mod queries;
pub use footprint::Footprint;
pub use ids::{FileId, HeadId, NodeId, RepoId};
pub use queries::Wait;

#[cfg(kani)]
mod proofs;

/// Starting credit is the request's size in leaves plus this reserve.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const RESERVE: u32 = 3;
/// Rechecks after clean rebases that cost nothing, per leaf.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const FREE_RECHECKS: u8 = 3;

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Outcome {
    Merged,
    Stopped,
    Replaced,
    Duplicate,
    AlreadyDone,
    NotActionable,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum EarlyEnd {
    Duplicate,
    AlreadyDone,
    NotActionable,
}

impl EarlyEnd {
    kernel_fn! {
    /// Ingest early-end rows / #771: preserve the recorded terminal outcome.
    pub fn outcome(self) -> (result: Outcome) [
        ensures result == match self {
            EarlyEnd::Duplicate => Outcome::Duplicate,
            EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
            EarlyEnd::NotActionable => Outcome::NotActionable,
        },
    ] {
        match self {
            EarlyEnd::Duplicate => Outcome::Duplicate,
            EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
            EarlyEnd::NotActionable => Outcome::NotActionable,
        }
    }
    }
}

/// The holding phase a leaf waiting in Ready goes back to.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Resume {
    Making,
    Checking,
    Reviewing,
    Publishing,
    Rebasing,
}

impl Resume {
    kernel_fn! {
        pub fn phase(self) -> (result: Phase) [
            ensures result == resumed_phase(self),
        ] {
            match self {
                Resume::Making => Phase::Making,
                Resume::Checking => Phase::Checking,
                Resume::Reviewing => Phase::Reviewing,
                Resume::Publishing => Phase::Publishing,
                Resume::Rebasing => Phase::Rebasing,
            }
        }
    }
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Phase {
    Ingesting,
    Auditing,
    Unplanned,
    Planning { must_split: bool },
    Ready(Resume),
    Making,
    Checking,
    Reviewing,
    Publishing,
    Queued,
    Rebasing,
    AwaitingRob,
    Stopping(Outcome),
    Closed(Outcome),
}

impl Phase {
    kernel_fn! {
        /// The phases in which a leaf holds a slot and its files.
        pub fn holding(self) -> (result: bool) [
            ensures result == holding(self),
        ] {
            matches!(
                self,
                Phase::Making
                    | Phase::Checking
                    | Phase::Reviewing
                    | Phase::Publishing
                    | Phase::Queued
                    | Phase::Rebasing
            )
        }
    }

    kernel_fn! {
        /// Where a holding leaf picks up again after waiting in Ready.
        pub fn resume(self) -> (result: Option<Resume>) [
            ensures result == match self {
                Phase::Making => Some(Resume::Making),
                Phase::Checking => Some(Resume::Checking),
                Phase::Reviewing => Some(Resume::Reviewing),
                Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
                Phase::Rebasing => Some(Resume::Rebasing),
                _ => None,
            },
        ] {
            match self {
                Phase::Making => Some(Resume::Making),
                Phase::Checking => Some(Resume::Checking),
                Phase::Reviewing => Some(Resume::Reviewing),
                Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
                Phase::Rebasing => Some(Resume::Rebasing),
                _ => None,
            }
        }
    }

    kernel_fn! {
        pub fn is_closed(self) -> (result: bool) [
            ensures result == (self is Closed),
        ] {
            matches!(self, Phase::Closed(_))
        }
    }

    kernel_fn! {
        fn is_stopping(self) -> (result: bool) [
            ensures result == (self is Stopping),
        ] {
            matches!(self, Phase::Stopping(_))
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Phase::Ingesting => "Ingesting",
            Phase::Auditing => "Auditing",
            Phase::Unplanned => "Unplanned",
            Phase::Planning { .. } => "Planning",
            Phase::Ready(_) => "Ready",
            Phase::Making => "Making",
            Phase::Checking => "Checking",
            Phase::Reviewing => "Reviewing",
            Phase::Publishing => "Publishing",
            Phase::Queued => "Queued",
            Phase::Rebasing => "Rebasing",
            Phase::AwaitingRob => "AwaitingRob",
            Phase::Stopping(_) => "Stopping",
            Phase::Closed(_) => "Closed",
        }
    }
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Role {
    Ingest,
    Audit,
    Plan,
    Make,
    Review,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum EffectKind {
    /// Start an agent run. Its reply, not its acknowledgement, moves the node on.
    StartRun(Role),
    /// Submit the check job to PrismaBuild.
    SubmitChecks,
    /// Start rebasing the branch onto main.
    StartRebase,
    /// Create the branch from main and the worktree, if they're missing.
    PrepareWorktree,
    Push,
    PostApproval,
    OpenPr,
    Enqueue,
    StopRun,
    WithdrawJob,
    ClosePr,
    /// Remove the worktree, then delete the branch.
    RemoveWorktree,
}

impl EffectKind {
    kernel_fn! {
        /// Effects that start an operation. The operation's reply moves the node on.
        fn starts_operation(self) -> (result: bool) [
            ensures result == matches!(self, EffectKind::StartRun(_)
                | EffectKind::SubmitChecks | EffectKind::StartRebase),
        ] {
            matches!(
                self,
                EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase
            )
        }
    }
}

/// The one effect a node may have in flight.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct InFlight {
    pub seq: u64,
    pub kind: EffectKind,
    /// The node has moved on. The acknowledgement only settles the effect.
    pub cancelling: bool,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum QKind {
    AuditDisagreement,
    NeedsSplitting,
    CantMeet,
    OutOfCredit,
    EffectFailed,
    CleanupFailed,
}

/// The step whose failure stopped a request for TL3 review under `fail_fast`
/// (Q54, Rob, 2026-11-02).
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum ReviewStop {
    ChecksFailed,
    ChangesRequested,
    CiFailed,
    MakerCantMeet,
    MakerNeedsFiles,
    MakerOutOfRoom,
    ReviewerOutOfRoom,
    PlanCantMeet,
    PublicationBody,
}

/// Where a node goes when Rob says carry on.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Cont {
    Ingesting,
    Auditing,
    Unplanned,
    Planning { must_split: bool },
    Ready(Resume),
    Stop(Outcome),
    Nowhere,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Question {
    pub id: u64,
    pub kind: QKind,
    pub cont: Cont,
    /// Credit the continuation needs.
    pub cost: u8,
    pub clarified: bool,
}

/// The request's credit ledger. The balance is derived, so it can't drift from the entries.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Credit {
    pub ready: bool,
    pub start: u32,
    pub earned: u32,
    pub added: u32,
    pub spent: u32,
    pub merge_credited: bool,
    /// Declared prerequisites withhold the ledger until planning admission.
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub withheld: bool,
}

impl Credit {
    kernel_fn! {
        pub fn balance(&self) -> (balance: i64) [
            ensures balance as int == if self.withheld { 0int } else {
                self.start as int + self.earned as int + self.added as int - self.spent as int
            },
        ] {
            if self.withheld {
                0
            } else {
                self.ledger_balance()
            }
        }
    }

    kernel_fn! {
        fn ledger_balance(&self) -> (balance: i64) [
            ensures balance as int == self.start as int + self.earned as int
                + self.added as int - self.spent as int,
        ] {
            self.start as i64 + self.earned as i64 + self.added as i64 - self.spent as i64
        }
    }

    kernel_fn! {
    /// Q18 / #771: compare spendable credit, including dependency withholding.
    fn can_spend(&self, cost: u32) -> (result: bool) [
        ensures result == (!self.withheld && self.start as int + self.earned as int
            + self.added as int >= self.spent as int + cost as int
            || self.withheld && cost == 0),
    ] {
        self.balance() >= cost as i64
    }
    }

    kernel_fn! {
        /// State-table credit ledger: one unit per paid retry or continuation.
        fn spend(&mut self, cost: u32) -> (result: ()) [
            requires old(self).spent as int + cost as int <= u32::MAX as int,
            ensures final(self).spent as int == old(self).spent as int + cost as int,
                final(self).start == old(self).start,
                final(self).earned == old(self).earned,
                final(self).added == old(self).added,
                final(self).withheld == old(self).withheld,
        ] {
            self.spent += cost;
        }
    }

    kernel_fn! {
        /// State-table credit ledger: Rob's answers top up the shortfall first.
        /// #771: consistent ledgers are nonnegative; withholding permits only
        /// zero-cost continuations. Hence automatic top-up adds at most cost.
        fn pay(&mut self, cost: u32) -> (result: ()) [
            requires
                old(self).start as int + old(self).earned as int + old(self).added as int
                    >= old(self).spent as int,
                old(self).withheld ==> cost == 0,
                old(self).added as int + (if cost as int > (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) { cost as int - (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) } else { 0int }) <= u32::MAX as int,
                old(self).spent as int + cost as int <= u32::MAX as int,
            ensures
                final(self).added as int == old(self).added as int + (if cost as int > (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) { cost as int - (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) } else { 0int }),
                final(self).ready == old(self).ready,
                final(self).merge_credited == old(self).merge_credited,
                final(self).added as int >= old(self).added as int,
                final(self).added as int <= old(self).added as int + cost as int,
                final(self).spent as int == old(self).spent as int + cost as int,
                final(self).start == old(self).start,
                final(self).earned == old(self).earned,
                final(self).withheld == old(self).withheld,
                final(self).start as int + final(self).earned as int + final(self).added as int
                    >= final(self).spent as int,
        ] {
            let short = cost as i64 - self.balance();
            if short > 0 {
                self.added += short as u32;
            }
            self.spent += cost;
        }
    }
    kernel_fn! {
        /// Q18 / #771: a u8 AddCredit answer and its automatic top-up together
        /// add at most 255. Valid questions cost at most one credit.
        fn fund_and_pay(&mut self, deposit: u8, cost: u8) -> (result: ()) [
            requires cost <= 1,
                old(self).start as int + old(self).earned as int + old(self).added as int
                    >= old(self).spent as int,
                old(self).withheld ==> cost == 0,
                old(self).added as int + 255 <= u32::MAX as int,
                old(self).spent as int + cost as int <= u32::MAX as int,
            ensures final(self).added as int >= old(self).added as int,
                final(self).added as int <= old(self).added as int + 255,
                final(self).spent as int == old(self).spent as int + cost as int,
                final(self).start == old(self).start, final(self).earned == old(self).earned,
                final(self).ready == old(self).ready, final(self).withheld == old(self).withheld,
                final(self).merge_credited == old(self).merge_credited,
                final(self).start as int + final(self).earned as int + final(self).added as int
                    >= final(self).spent as int,
                final(self).added as int == old(self).added as int + deposit as int
                    + if cost as int > old(self).start as int + old(self).earned as int
                        + old(self).added as int + deposit as int - old(self).spent as int {
                        cost as int - (old(self).start as int + old(self).earned as int
                            + old(self).added as int + deposit as int - old(self).spent as int)
                    } else { 0int },
        ] {
            self.added += deposit as u32;
            self.pay(cost as u32);
        }
    }
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Node {
    pub phase: Phase,
    /// Position in the phase's agenda.
    pub pos: u8,
    pub repo: RepoId,
    /// The files this leaf may change.
    pub footprint: Footprint,
    /// Operation generation. Every phase change and retry takes a new one.
    pub generation: u64,
    pub next_seq: u64,
    pub effect: Option<InFlight>,
    /// Generation of an agent run or rebase that may be running.
    pub run: Option<u64>,
    /// Generation of a check job that may be running.
    pub job: Option<u64>,
    /// Holds an agent-pool grant.
    pub pool: bool,
    /// Place in line for a slot, or for the agent pool.
    pub ticket: Option<u64>,
    /// Holds a slot and its files.
    pub holds: bool,
    pub paused: bool,
    pub last_cmd: u64,
    pub question: Option<Question>,
    pub attempt: u8,
    pub round: u8,
    pub replanned: bool,
    pub plan_retries: u8,
    pub audit_rounds: u8,
    pub early_end: Option<EarlyEnd>,
    pub run_failures: u8,
    pub run_incidents: u8,
    pub fx_failures: u8,
    pub fx_incidents: u8,
    pub waiting_incident: bool,
    pub incident_needs_rob: bool,
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub proposed: Option<HeadId>,
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub adopted: bool,
    pub head: Option<HeadId>,
    /// The head that passed its checks.
    pub checked: Option<HeadId>,
    /// The exact head that passed checks and review.
    pub approved: Option<HeadId>,
    pub merged: Option<HeadId>,
    pub pr_open: bool,
    pub had_pr: bool,
    /// The branch and worktree may exist.
    pub worktree: bool,
    pub rechecks: u8,
    /// This review has used its fresh conversation after exhaustion.
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub review_restarted: bool,
    pub credit: Credit,
    /// Orders agent-pool grants (Q39). Unranked keeps the older row shape.
    #[cfg_attr(
        all(feature = "serde", not(kani)),
        serde(default, skip_serializing_if = "Priority::is_unranked")
    )]
    pub priority: Priority,
    /// Set when `fail_fast` stopped the node for TL3 review instead of
    /// retrying (Q54). A postmortem for it is owed as `needed_fix`.
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub review_stop: Option<ReviewStop>,
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub planning_started: bool,
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub waiting_prerequisite: Option<u64>,
}

impl Node {
    kernel_fn! {
        /// #771: allocate a fresh command ID without wrapping at exhaustion.
        pub fn next_command_id(&self) -> (id: Option<u64>) [
            ensures
                match id {
                    None => self.last_cmd == u64::MAX,
                    Some(next) => self.last_cmd < u64::MAX
                        && next as int == self.last_cmd as int + 1,
                },
        ] {
            self.last_cmd.checked_add(1)
        }
    }
}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: constructor facts used by the executable apply composition.
pub closed spec fn initial_node(n: Node, repo: RepoId) -> bool {
    n.repo == repo && n.phase == Phase::Ingesting && n.pos == 0
        && n.generation == 0 && n.next_seq == 0 && n.last_cmd == 0
        && n.credit.start == 0 && n.credit.earned == 0 && n.credit.added == 0 && n.credit.spent == 0
        && !n.credit.ready && !n.credit.merge_credited && !n.credit.withheld
        && n.effect.is_none() && n.run.is_none() && n.job.is_none() && n.ticket.is_none()
        && n.question.is_none() && !n.pool && !n.holds && !n.paused && !n.waiting_incident
        && n.run_failures == 0 && n.fx_failures == 0 && n.run_incidents == 0 && n.fx_incidents == 0
        && n.footprint == footprint::inline_footprint(0)
}
}

kernel_fn! {
impl Node {
    /// #771: new requests begin with zero ledger counters and an inline empty set.
    pub fn new(repo: RepoId) -> (result: Node) [
        ensures initial_node(result, repo),
    ] {
        kernel_proof! { reveal(initial_node); }
        Node {
            phase: Phase::Ingesting,
            pos: 0,
            repo,
            footprint: Footprint::inline(0),
            generation: 0,
            next_seq: 0,
            effect: None,
            run: None,
            job: None,
            pool: false,
            ticket: None,
            holds: false,
            paused: false,
            last_cmd: 0,
            question: None,
            attempt: 0,
            round: 0,
            replanned: false,
            plan_retries: 0,
            audit_rounds: 0,
            early_end: None,
            run_failures: 0,
            run_incidents: 0,
            fx_failures: 0,
            fx_incidents: 0,
            waiting_incident: false,
            incident_needs_rob: false,
            proposed: None,
            adopted: false,
            head: None,
            checked: None,
            approved: None,
            merged: None,
            pr_open: false,
            had_pr: false,
            worktree: false,
            rechecks: 0,
            review_restarted: false,
            credit: Credit {
                ready: false,
                start: 0,
                earned: 0,
                added: 0,
                spent: 0,
                merge_credited: false,
                withheld: false,
            },
            priority: Priority::Unranked,
            review_stop: None,
            planning_started: false,
            waiting_prerequisite: None,
        }
    }
}
}

impl Node {
    kernel_fn! {
        fn current(&self) -> (result: Option<Item>) [
            ensures result == current_item(*self),
        ] {
            agenda(self.phase).get(self.pos as usize).copied()
        }
    }

    kernel_fn! {
        /// A paused leaf whose next step would start work, and which settle will
        /// move to Ready instead.
        fn pause_move_due(&self) -> (result: bool) [
            ensures result == pause_due(*self),
        ] {
            self.paused
                && self.phase.holding()
                && !self.waiting_incident
                && self.effect.is_none()
                && matches!(
                    self.current(),
                    Some(Item::Do(_) | Item::Start(_) | Item::StartChecks | Item::StartRebase)
                )
        }
    }
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Gate {
    pub w: u8,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    pub nodes: Vec<Node>,
    /// One gate per repo.
    pub gates: Vec<Gate>,
    pub pool_capacity: u8,
    pub next_ticket: u64,
    pub next_question: u64,
    file_sets: Vec<Vec<FileId>>,
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    dependencies: Vec<Dependency>,
    /// Q54: a step that would retry the work stops the request for TL3
    /// review instead. Config sets it at every open, so it applies to open
    /// requests too.
    #[cfg_attr(all(feature = "serde", not(kani)), serde(default))]
    pub fail_fast: bool,
}

kernel_fn! {
/// #771: an observational world snapshot, including every interned file list.
/// Copy fields keep their values; nested vectors keep their element sequences.
fn snapshot_world(world: &World) -> (result: World) [
    ensures (settle_ready(*world) ==> settle_ready(result)),
        result.nodes@ == world.nodes@, result.gates@ == world.gates@,
        result.dependencies@ == world.dependencies@,
        result.file_sets@.len() == world.file_sets@.len(),
        forall|i: int| 0 <= i < world.file_sets@.len()
            ==> result.file_sets@[i]@ == world.file_sets@[i]@,
        result.pool_capacity == world.pool_capacity,
        result.next_ticket == world.next_ticket, result.next_question == world.next_question,
        result.fail_fast == world.fail_fast,
] {
    let mut file_sets: Vec<Vec<FileId>> = Vec::new();
    let mut index = 0usize;
    kernel_while! { (index < world.file_sets.len()) [
        invariant index <= world.file_sets.len(), file_sets@.len() == index,
            forall|i: int| 0 <= i < index ==> file_sets@[i]@ == world.file_sets@[i]@,
        decreases world.file_sets.len() - index,
    ] {
        file_sets.push(world.file_sets[index].clone());
        index += 1;
    } }
    let result = World {
        nodes: world.nodes.clone(), gates: world.gates.clone(),
        dependencies: world.dependencies.clone(), file_sets,
        pool_capacity: world.pool_capacity, next_ticket: world.next_ticket,
        next_question: world.next_question, fail_fast: world.fail_fast,
    };
    kernel_proof! {
        if settle_ready(*world) {
            assert forall|j: int| 0 <= j < result.nodes@.len() implies
                footprint::valid_footprint(result, result.nodes@[j].footprint) by {
                footprint::valid_under_table_growth(*world, result, result.nodes@[j].footprint);
            }
            reveal(footprint::ordered_table);
        }
    }
    result
}
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Reply {
    IngestDone {
        size: u8,
    },
    IngestEarlyEnd(EarlyEnd),
    AuditAgrees,
    AuditDisagrees,
    PlanLeaf {
        footprint: Footprint,
        fits: bool,
    },
    PlanSplit,
    PlanCantMeet,
    ProtectedPaths,
    MakerDone {
        head: HeadId,
    },
    MakerNeedsFiles {
        files: Footprint,
        fits: bool,
    },
    MakerCantMeet,
    MakerOutOfRoom,
    ChecksDone {
        report: bool,
        pass: bool,
        in_footprint: bool,
    },
    Approved {
        head: HeadId,
    },
    ChangesRequested,
    ReviewerOutOfRoom,
    RebaseClean {
        head: HeadId,
    },
    RebaseNeedsChanges,
    /// A crash, setup error, lost worker or stall.
    RunFailed,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Ack {
    Confirmed,
    Retryable,
    Permanent,
    /// A TL3-created local body defect; only OpenPr can stop for this cause.
    BodyPolicyRefused,
}

/// A request's priority from its issue (Q39): best first, then unranked.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
    #[default]
    Unranked,
}

impl Priority {
    pub fn is_unranked(&self) -> bool {
        *self == Priority::Unranked
    }

    kernel_fn! {
    /// Q39 / #771: 0 for P0 through 4 for unranked, in pool priority order.
    pub fn rank(self) -> (result: u8) [
        ensures result == match self {
            Priority::P0 => 0u8, Priority::P1 => 1u8, Priority::P2 => 2u8,
            Priority::P3 => 3u8, Priority::Unranked => 4u8,
        },
    ] {
        self as u8
    }
    }
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum PrerequisiteKind {
    Issue,
    PullRequest,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct DependencyTarget {
    pub kind: PrerequisiteKind,
    pub number: u64,
    pub request: Option<NodeId>,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum DependencyStatus {
    Pending,
    Delivered,
    Abandoned,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Fact {
    DependencyRemove {
        kind: PrerequisiteKind,
        number: u64,
    },
    Dependency {
        target: DependencyTarget,
        status: DependencyStatus,
    },
    PrMerged {
        head: HeadId,
        checks_passed: bool,
    },
    PrClosedByPerson,
    PrDequeuedByPerson,
    PushByOther {
        head: HeadId,
    },
    CiFailed {
        infra: bool,
    },
    Conflict,
    IncidentCleared {
        by_rob: bool,
    },
    /// A PR or queue record is alive but unchanged across allowance looks.
    UpstreamStalled,
    /// The issue's title or labels now give this priority.
    Priority(Priority),
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Answer {
    Continue,
    TryAgain,
    AddCredit(u8),
    ChangeRequest,
    Stop,
    Unclear,
    Retry,
    MarkHandled,
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Command {
    Stop,
    Pause,
    Resume,
    ChangeRequest,
    Answer { question: u64, answer: Answer },
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
// A plain tag. Without it, Q39's Priority field let rustc hide the event's
// discriminant in that field's spare values, and decoding it at every match
// took the reply proofs from 15,166 VCCs to 19,549 and past 44 GB. With the
// tag, apply_maker_done has 4,612.
#[repr(u8)]
pub enum Event {
    NewRequest {
        repo: RepoId,
        /// Events stored before Q39 carry none and read as unranked; unranked
        /// serializes that way too, as #371 requires of later fields.
        #[cfg_attr(
            all(feature = "serde", not(kani)),
            serde(default, skip_serializing_if = "Priority::is_unranked")
        )]
        priority: Priority,
    },
    NewAdoption {
        repo: RepoId,
        head: HeadId,
        #[cfg_attr(
            all(feature = "serde", not(kani)),
            serde(default, skip_serializing_if = "Priority::is_unranked")
        )]
        priority: Priority,
    },
    Reply {
        node: NodeId,
        generation: u64,
        reply: Reply,
    },
    Ack {
        node: NodeId,
        seq: u64,
        ack: Ack,
    },
    Fact {
        node: NodeId,
        fact: Fact,
    },
    Command {
        node: NodeId,
        id: u64,
        command: Command,
    },
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    UnapprovedMerge,
    Replanned,
    PrClosed,
    PrDequeued,
    OutsidePush,
    LateMerge,
    DependencyAbandoned { number: u64 },
    DependencyCycle { number: u64 },
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Ask { question: u64 },
    MeasuresPosted,
    Summary(Outcome),
    FileReplacement,
    Incident { needs_rob: bool },
    Note(NoteKind),
}

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    Effect {
        node: NodeId,
        seq: u64,
        kind: EffectKind,
        head: Option<HeadId>,
    },
    Notice {
        node: NodeId,
        notice: Notice,
    },
}

/// Most outputs one transition emits. A late merge with an unapproved head
/// emits four notices, the most of any transition.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const OUTBOX: usize = 5;

/// The outputs of one transition: one event applied, or one internal
/// transition of settle. A fixed array holds them, not a list on the heap,
/// because Kani reasons about a growable list far more expensively: the merge
/// proofs ran out of memory reading one. `step` collects them into its list.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outbox {
    items: [Option<Out>; OUTBOX],
    len: usize,
}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: public specification without exposing the private output storage.
pub closed spec fn empty_outbox(out: Outbox) -> bool {
    out.len == 0 && out.items@ == Seq::new(OUTBOX as nat, |i: int| None)
}
}

kernel_fn! {
impl Outbox {
    /// #771: the fixed output buffer begins with no emitted entries.
    pub const fn new() -> (result: Outbox) [
        ensures empty_outbox(result),
    ] {
        kernel_proof! { reveal(empty_outbox); }
        Outbox {
            items: [None; OUTBOX],
            len: 0,
        }
    }
}
}

impl Outbox {
    kernel_fn! {
        fn push(&mut self, o: Out) -> (result: ()) [
        requires old(self).len < OUTBOX,
        ensures final(self).len == old(self).len + 1,
            final(self).items@ == old(self).items@.update(old(self).len as int, Some(o)),
        ] {
                assert!(
                    self.len < OUTBOX,
                    "a transition emitted more than OUTBOX outputs"
                );
                self.items[self.len] = Some(o);
                self.len += 1;
            }
    }

    /// The output in slot `i`, if any. Slots fill in order.
    pub fn get(&self, i: usize) -> Option<Out> {
        self.items[i]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether this transition emitted `o`.
    pub fn contains(&self, o: Out) -> bool {
        let mut i = 0;
        while i < OUTBOX {
            if self.items[i] == Some(o) {
                return true;
            }
            i += 1;
        }
        false
    }

    kernel_fn! {
    /// #771: append occupied slots in the same order as iter.flatten.copied.
    fn drain_into(&self, out: &mut Vec<Out>) -> (result: ()) [
    ] {
        let mut index = 0usize;
        kernel_while! { (index < OUTBOX) [
            invariant index <= OUTBOX,
            decreases OUTBOX - index,
        ] {
            if let Some(item) = self.items[index] { out.push(item); }
            index += 1;
        } }
    }
    }
}

/// One step of a phase's agenda.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// Emit the effect and wait for its acknowledgement.
    Do(EffectKind),
    /// Take a place in the agent pool, then start the run.
    Start(Role),
    StartChecks,
    StartRebase,
    /// Wait for the run's or job's reply.
    Await,
    /// Wait for planning admission.
    Admit,
    /// Wait for a slot and its files.
    Slot,
    /// Wait in Mergify's queue.
    Merge,
    /// Wait for Rob's answer.
    Answer,
    /// Give back the slot and files.
    Release,
    /// A teardown effect, skipped when its target is already gone.
    Teardown(EffectKind),
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
const RELEASE_STEP: u8 = 3;

kernel_fn! {
    fn agenda(phase: Phase) -> (result: &'static [Item]) [
        ensures result@ == items(phase), result.len() <= 5,
    ] {
        use EffectKind as K;
        use Item as I;
        match phase {
            Phase::Ingesting => &[I::Start(Role::Ingest), I::Await],
            Phase::Auditing => &[I::Start(Role::Audit), I::Await],
            Phase::Unplanned => &[I::Admit],
            Phase::Planning { .. } => &[I::Start(Role::Plan), I::Await],
            Phase::Ready(_) => &[I::Slot],
            Phase::Making => &[I::Do(K::PrepareWorktree), I::Start(Role::Make), I::Await],
            Phase::Checking => &[I::StartChecks, I::Await],
            Phase::Reviewing => &[I::Start(Role::Review), I::Await],
            Phase::Publishing => &[
                I::Do(K::Push),
                I::Do(K::PostApproval),
                I::Do(K::OpenPr),
                I::Do(K::Enqueue),
            ],
            Phase::Queued => &[I::Merge],
            Phase::Rebasing => &[I::StartRebase, I::Await],
            Phase::AwaitingRob => &[I::Answer],
            Phase::Stopping(_) => &[
                I::Teardown(K::StopRun),
                I::Teardown(K::WithdrawJob),
                I::Teardown(K::ClosePr),
                I::Release,
                I::Teardown(K::RemoveWorktree),
            ],
            Phase::Closed(_) => &[],
        }
    }
}

kernel_fn! {
/// #771: a retried run starts at its original operation agenda entry.
fn operation_start(phase: Phase) -> (result: u8) [
    ensures result == if phase == Phase::Making { 1u8 } else { 0u8 },
] {
    match phase {
        Phase::Making => 1,
        _ => 0,
    }
}
}

kernel_fn! {
    fn rank(phase: Phase) -> (result: u64) [
        ensures result as int == phase_rank(phase), result <= 32,
        decreases if phase is Ready { 1nat } else { 0nat },
    ] {
        match phase {
            Phase::Closed(_) => 0,
            Phase::Stopping(_) => 2,
            Phase::Queued => 4,
            Phase::Publishing => 6,
            Phase::Reviewing => 10,
            Phase::Checking => 14,
            Phase::Rebasing => 18,
            Phase::Making => 22,
            Phase::Ready(r) => rank(r.phase()) + 2,
            Phase::Planning { .. } => 26,
            Phase::Unplanned => 28,
            Phase::Auditing => 30,
            Phase::Ingesting => 32,
            Phase::AwaitingRob => 3,
        }
    }
}

kernel_fn! {
    fn cont_rank(cont: Cont) -> (result: u64) [
        ensures result as int == continuation_rank(cont), result <= 32,
    ] {
        match cont {
            Cont::Ingesting => 32,
            Cont::Auditing => 30,
            Cont::Unplanned => 28,
            Cont::Planning { .. } => 26,
            Cont::Ready(r) => rank(Phase::Ready(r)),
            Cont::Stop(_) | Cont::Nowhere => 2,
        }
    }
}

kernel_fn! {
    fn notice(out: &mut Outbox, node: usize, value: Notice) -> (result: ()) [
    requires old(out).len < OUTBOX,
    ensures final(out).len == old(out).len + 1,
    ] {
        out.push(Out::Notice {
            node: NodeId(node as u64),
            notice: value,
        });
    }
}

impl World {
    pub fn new(gates: &[u8], pool_capacity: u8) -> World {
        World {
            nodes: Vec::new(),
            gates: gates.iter().map(|&w| Gate { w }).collect(),
            pool_capacity,
            next_ticket: 0,
            next_question: 0,
            file_sets: Vec::new(),
            dependencies: Vec::new(),
            fail_fast: false,
        }
    }

    kernel_fn! {
    /// #771: external node IDs name existing storage or are ignored.
    fn index(&self, node: NodeId) -> (result: Option<usize>) [
        ensures result.is_some() == ((node.0 as int) < self.nodes@.len()),
            match result {
                Some(i) => i as int == node.0 as int && i < self.nodes@.len(),
                None => true,
            },
    ] {
        let length = self.nodes.len();
        let i = node.index()?;
        if i < length { Some(i) } else { None }
    }
    }

    /// The sum of every node's distance to Closed.
    pub fn measure(&self) -> u64 {
        let mut total = 0;
        for n in &self.nodes {
            total += n.distance();
        }
        total += self
            .dependencies
            .iter()
            .filter(|d| {
                d.status != DependencyStatus::Delivered
                    && !self.nodes[d.node.0 as usize].phase.is_closed()
            })
            .count() as u64;
        total
    }

    /// True when settle has nothing left to do.
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
            if advance(&mut w, i, &mut out) { return false; }
            i += 1;
        } }
        true
    }
    }

    /// One outside event that moves the world closer to done: a successful
    /// reply, a confirmation, a fact, or one of Rob's answers. Kani proves it
    /// always exists while a node is open, and that it lowers `measure`.
    pub fn witness(&self) -> Option<Event> {
        for (i, n) in self.nodes.iter().enumerate() {
            if n.phase.is_closed() {
                continue;
            }
            let node = NodeId(i as u64);
            if n.paused && !n.phase.is_stopping() {
                return Some(Event::Command {
                    node,
                    id: n.next_command_id()?,
                    command: Command::Resume,
                });
            }
            if let Some(e) = n.effect {
                return Some(Event::Ack {
                    node,
                    seq: e.seq,
                    ack: Ack::Confirmed,
                });
            }
            if n.waiting_incident {
                return Some(Event::Fact {
                    node,
                    fact: Fact::IncidentCleared { by_rob: true },
                });
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
                return Some(Event::Command {
                    node,
                    id: n.next_command_id()?,
                    command,
                });
            }
            if n.phase == Phase::Unplanned && !self.dependencies_delivered(node) {
                let dependency = self
                    .dependencies
                    .iter()
                    .find(|d| {
                        d.node == node && (d.rejected || d.status != DependencyStatus::Delivered)
                    })
                    .unwrap();
                if dependency.rejected || dependency.status == DependencyStatus::Abandoned {
                    return Some(Event::Command {
                        node,
                        id: n.next_command_id()?,
                        command: Command::ChangeRequest,
                    });
                }
                return Some(Event::Fact {
                    node,
                    fact: Fact::Dependency {
                        target: dependency.target,
                        status: DependencyStatus::Delivered,
                    },
                });
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
                        _ => continue,
                    };
                    return Some(Event::Reply {
                        node,
                        generation,
                        reply,
                    });
                }
                Some(Item::Merge) => {
                    let head = n.head.unwrap_or(HeadId(0));
                    let fact = Fact::PrMerged {
                        head,
                        checks_passed: true,
                    };
                    return Some(Event::Fact { node, fact });
                }
                // Waiting on the gate or the pool: another node holds what it needs.
                _ => continue,
            }
        }
        None
    }

    /// Everything the kernel promises about a world at rest: consistent and settled.
    pub fn valid(&self) -> bool {
        self.consistent() && self.is_settled()
    }

    /// What holds between any two transitions, including inside settle. Kani
    /// proves that applying an event and every internal transition keep it.
    kernel_fn! {
    /// #771: successful executable consistency checking supplies the exact
    /// progress invariants used by the source-connected theorem.
    pub fn consistent(&self) -> (result: bool) [
        requires world_headroom(*self), footprint::ordered_table(*self),
        ensures result ==> world_checked(*self),
    ] {
        self.check().is_ok()
    }
    }

    /// `consistent`, naming the first rule a world breaks.
    kernel_fn! {
    /// #771: the real world checker establishes positive capacities and the
    /// per-node and prerequisite facts, without changing any acceptance rule.
    pub fn check(&self) -> (result: Result<(), Broken>) [
        requires world_headroom(*self), footprint::ordered_table(*self),
        ensures result.is_ok() ==> world_checked(*self),
    ] proof [reveal(world_headroom); reveal(world_checked);] {
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

    /// What a leaf needs before it may resume into a holding phase.
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

/// The rule a world breaks, from `World::check`.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    Config,
    Footprint,
    Agenda,
    Effect,
    Holds,
    Run,
    Pool,
    Ticket,
    Question,
    Incident,
    Credit,
    Leaf,
    Pr,
    Teardown,
    Merge,
    Closed,
    Gate,
    ReviewStop,
    Dependencies,
}

// proof map start
/// The Kani proof that covers each event variant. The match is exhaustive, so
/// a new variant without a proof doesn't compile, and a test checks that each
/// named proof exists.
pub fn proof_of_event(event: &Event) -> &'static str {
    match event {
        Event::NewRequest { .. } => "apply_new_request",
        Event::NewAdoption { .. } => "apply_new_adoption",
        Event::Reply { reply, .. } => match reply {
            Reply::IngestDone { .. } => "apply_ingest_done",
            Reply::IngestEarlyEnd(_) => "apply_ingest_early_end",
            Reply::AuditAgrees => "apply_audit_agrees",
            Reply::AuditDisagrees => "apply_audit_disagrees",
            Reply::PlanLeaf { .. } => "apply_plan_leaf",
            Reply::PlanSplit => "apply_plan_split",
            Reply::PlanCantMeet => "apply_plan_cant_meet",
            Reply::ProtectedPaths => "apply_protected_paths",
            Reply::MakerDone { .. } => "apply_maker_done",
            Reply::MakerNeedsFiles { .. } => "apply_maker_needs_files",
            Reply::MakerCantMeet => "apply_maker_cant_meet",
            Reply::MakerOutOfRoom => "apply_maker_out_of_room",
            Reply::ChecksDone { .. } => "apply_checks_done",
            Reply::Approved { .. } => "apply_approved",
            Reply::ChangesRequested => "apply_changes_requested",
            Reply::ReviewerOutOfRoom => "apply_reviewer_out_of_room",
            Reply::RebaseClean { .. } => "apply_rebase_clean",
            Reply::RebaseNeedsChanges => "apply_rebase_needs_changes",
            Reply::RunFailed => "apply_run_failed",
        },
        Event::Ack { ack, .. } => match ack {
            Ack::Confirmed => "apply_ack_confirmed",
            Ack::Retryable => "apply_ack_retryable",
            Ack::Permanent => "apply_ack_permanent",
            Ack::BodyPolicyRefused => "apply_ack_body_policy_refused",
        },
        Event::Fact { fact, .. } => match fact {
            // Merges are heavy, so they get one proof per phase: pr_merged_<phase>.
            Fact::PrMerged { .. } => "pr_merged_by_phase",
            Fact::PrClosedByPerson => "apply_pr_closed",
            Fact::PrDequeuedByPerson => "apply_pr_dequeued",
            Fact::PushByOther { .. } => "apply_outside_push",
            Fact::CiFailed { .. } => "apply_ci_failed",
            Fact::Conflict => "apply_conflict",
            Fact::IncidentCleared { .. } => "apply_incident_cleared",
            Fact::UpstreamStalled => "apply_upstream_stalled",
            Fact::Priority(_) => "apply_priority",
            Fact::Dependency { .. } => "apply_dependency",
            Fact::DependencyRemove { .. } => "apply_dependency_remove",
        },
        Event::Command { command, .. } => match command {
            Command::Stop => "apply_stop",
            Command::Pause => "apply_pause",
            Command::Resume => "apply_resume",
            Command::ChangeRequest => "apply_change_request",
            Command::Answer { answer, .. } => match answer {
                Answer::Continue => "apply_answer_continue",
                Answer::TryAgain => "apply_answer_try_again",
                Answer::AddCredit(_) => "apply_answer_add_credit",
                Answer::ChangeRequest => "apply_answer_change_request",
                Answer::Stop => "apply_answer_stop",
                Answer::Unclear => "apply_answer_unclear",
                Answer::Retry => "apply_answer_retry",
                Answer::MarkHandled => "apply_answer_mark_handled",
            },
        },
    }
}

/// The Kani proofs that cover each phase: settle's internal transitions, and
/// progress. Exhaustive for the same reason.
pub fn proofs_of_phase(phase: Phase) -> &'static [&'static str] {
    match phase {
        Phase::Ingesting => &["advance_ingesting", "progress_from_ingesting"],
        Phase::Auditing => &["advance_auditing", "progress_from_auditing"],
        Phase::Unplanned => &["advance_unplanned", "progress_from_unplanned"],
        Phase::Planning { .. } => &["advance_planning", "progress_from_planning"],
        Phase::Ready(_) => &["advance_ready", "progress_from_ready"],
        Phase::Making => &[
            "advance_making",
            "progress_from_making",
            "apply_adoption_at_most_once",
            "apply_adoption_has_no_maker_credit",
        ],
        Phase::Checking => &["advance_checking", "progress_from_checking"],
        Phase::Reviewing => &["advance_reviewing", "progress_from_reviewing"],
        Phase::Publishing => &["advance_publishing", "progress_from_publishing"],
        Phase::Queued => &["advance_queued", "progress_from_queued"],
        Phase::Rebasing => &[
            "advance_rebasing",
            "progress_from_rebasing",
            "apply_rebase_progress_gate",
        ],
        Phase::AwaitingRob => &["advance_awaiting_rob", "progress_from_awaiting_rob"],
        Phase::Stopping(_) => &["advance_stopping", "progress_from_stopping"],
        Phase::Closed(_) => &["advance_closed", "progress_from_closed"],
    }
}
// proof map end

/// The kernel: apply one event, then settle.
kernel_fn! {
/// #771: the public executable step inherits the checked whole-step K bounds.
pub fn step(world: World, event: Event) -> (result: (World, Vec<Out>)) [
    requires step_scope(world),
    ensures step_budget(world, result.0),
] {
    let (world, out, _) = step_with_passes(world, event);
    (world, out)
}
}

/// Settle after the controller reconciles stored gate or agent capacity.
pub fn settle_world(mut world: World) -> (World, Vec<Out>) {
    let mut out = Vec::new();
    settle(&mut world, &mut out, None);
    (world, out)
}

/// `step`, also reporting how many passes settle needed. Tests use it to size
/// the loop bound that Kani's unwinding check then proves.
kernel_fn! {
/// #771: verify the production event-to-settle composition. The scope here is
/// the arithmetic and storage projection of consistency, plus headroom. The
/// progress theorem separately requires command freshness for every node.
pub fn step_with_passes(mut world: World, event: Event) -> (result: (World, Vec<Out>, u32)) [
    requires step_scope(world),
    ensures step_budget(world, result.0),
] {
    kernel_snapshot! { before_step = world }
    let mut outbox = Outbox::new();
    kernel_proof! { reveal(step_scope); reveal(empty_outbox); }
    apply(&mut world, event, &mut outbox);
    kernel_proof! {
        reveal(world_headroom);
        open_prefix_compare(before_step@.nodes@, world.nodes@, before_step@.nodes@.len());
        open_prefix_monotone(world.nodes@, before_step@.nodes@.len(), world.nodes@.len());
        settle_reserve_totals(world.nodes@);
        open_prefix_monotone(world.nodes@, 0, world.nodes@.len());
        reveal(open_prefix);
        assert forall|j: int| 0 <= j < world.nodes@.len() implies
            world.nodes@[j].generation as int + generation_capacity(world.nodes@[j]) <= u64::MAX as int
                && world.nodes@[j].next_seq as int + effect_capacity(world.nodes@[j]) <= u64::MAX as int
                && world.nodes@[j].credit.earned as int + merge_capacity(world.nodes@[j]) <= u32::MAX as int
        by { local_capacities(world.nodes@[j]); }
        assert(settle_ready(world));
    }
    kernel_snapshot! { after_apply = world }
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    let passes = settle(&mut world, &mut out, None);
    kernel_proof! {
        reveal(step_budget);
        assert forall|j: int| 0 <= j < before_step@.nodes@.len() implies
            world.nodes@[j].generation as int <= before_step@.nodes@[j].generation as int + GENERATION_STEP_BUDGET
                && world.nodes@[j].next_seq as int <= before_step@.nodes@[j].next_seq as int + EFFECT_STEP_BUDGET
                && world.nodes@[j].credit.start as int <= before_step@.nodes@[j].credit.start as int + CREDIT_START_STEP_BUDGET
                && world.nodes@[j].credit.earned as int <= before_step@.nodes@[j].credit.earned as int + CREDIT_EARNED_STEP_BUDGET
                && world.nodes@[j].credit.added as int <= before_step@.nodes@[j].credit.added as int + CREDIT_ADDED_STEP_BUDGET
                && world.nodes@[j].credit.spent as int <= before_step@.nodes@[j].credit.spent as int + CREDIT_SPENT_STEP_BUDGET
        by { local_capacities(after_apply@.nodes@[j]); }
    }
    (world, out, passes)
}
}

/// `step`, also returning the world after the event and after every internal
/// transition. Tests check each one.
pub fn step_traced(mut world: World, event: Event) -> (World, Vec<Out>, Vec<World>) {
    let mut outbox = Outbox::new();
    let mut trace = Vec::new();
    apply(&mut world, event, &mut outbox);
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    trace.push(snapshot_world(&world));
    settle(&mut world, &mut out, Some(&mut trace));
    (world, out, trace)
}

kernel_fn! {
/// #771: compose the actual event handlers' per-apply counter bounds.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn apply(w: &mut World, event: Event, out: &mut Outbox) -> (result: ()) [
    requires world_headroom(*old(w)), footprint::ordered_table(*old(w)),
        dependencies::edges_indexable(*old(w)), old(out).len + 4 <= OUTBOX,
        forall|j: int| 0 <= j < old(w).nodes@.len()
            ==> arithmetic_consistent(old(w).nodes@[j])
                && settle_shape(old(w).nodes@[j]) && stopping_effect_fits(old(w).nodes@[j])
                && (old(w).nodes@[j].phase is Closed ==> old(w).nodes@[j].effect.is_none())
                && footprint::valid_footprint(*old(w), old(w).nodes@[j].footprint),
    ensures forall|j: int| 0 <= j < final(w).nodes@.len() ==>
            settle_shape(final(w).nodes@[j])
                && footprint::valid_footprint(*final(w), final(w).nodes@[j].footprint),
        forall|j: int| 0 <= j < old(w).nodes@.len() ==>
            (old(w).nodes@[j].phase is Closed ==> final(w).nodes@[j].phase is Closed)
                && final(w).nodes@[j].repo == old(w).nodes@[j].repo,
        old(w).nodes@.len() <= final(w).nodes@.len() <= old(w).nodes@.len() + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@, final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        forall|j: int| 0 <= j < old(w).nodes@.len() ==>
            final(w).nodes@[j].generation as int <= old(w).nodes@[j].generation as int + 2
            && final(w).nodes@[j].next_seq as int <= old(w).nodes@[j].next_seq as int + 1
            && final(w).nodes@[j].credit.start as int <= old(w).nodes@[j].credit.start as int + CREDIT_START_STEP_BUDGET
            && final(w).nodes@[j].credit.earned as int + merge_capacity(final(w).nodes@[j])
                <= old(w).nodes@[j].credit.earned as int + 1
            && final(w).nodes@[j].credit.added as int <= old(w).nodes@[j].credit.added as int + CREDIT_ADDED_STEP_BUDGET
            && final(w).nodes@[j].credit.spent as int <= old(w).nodes@[j].credit.spent as int + CREDIT_SPENT_STEP_BUDGET,
        forall|j: int| old(w).nodes@.len() <= j < final(w).nodes@.len() ==>
            final(w).nodes@[j].generation == 0 && final(w).nodes@[j].next_seq == 0
            && final(w).nodes@[j].credit.start == 0 && final(w).nodes@[j].credit.earned == 0
            && final(w).nodes@[j].credit.added == 0 && final(w).nodes@[j].credit.spent == 0,
] proof [reveal(world_headroom); reveal(initial_node); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_apply = *w }
    match event {
        Event::NewAdoption {
            repo,
            head,
            priority,
        } => {
            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
                let mut node = Node::new(repo);
                node.proposed = Some(head);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::NewRequest { repo, priority } => {
            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {
                let mut node = Node::new(repo);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::Reply {
            node,
            generation,
            reply,
        } => {
            if let Some(i) = w.index(node) {
                on_reply(w, i, generation, reply, out);
            }
        }
        Event::Ack { node, seq, ack } => {
            if let Some(i) = w.index(node) {
                on_ack(w, i, seq, ack, out);
            }
        }
        Event::Fact { node, fact } => {
            if let Some(i) = w.index(node) {
                on_fact(w, i, fact, out);
            }
        }
        Event::Command { node, id, command } => {
            if let Some(i) = w.index(node) {
                on_command(w, i, id, command, out);
            }
        }
    }
    kernel_proof! {
        assert forall|j: int| 0 <= j < w.nodes@.len() implies
            footprint::valid_footprint(*w, w.nodes@[j].footprint) by {
            if j < before_apply@.nodes@.len() {
                if w.nodes@[j].footprint == before_apply@.nodes@[j].footprint {
                    footprint::valid_under_table_growth(before_apply@, *w, w.nodes@[j].footprint);
                }
            } else {
                footprint::inline_is_valid(*w, 0);
            }
        }
    }
}
}

// ---------------------------------------------------------------- transitions

kernel_fn! {
    /// Every phase change starts a new generation, and the node stops waiting on
    /// whatever effect it had in flight. #771: one generation increment.
    fn set_phase(m: &mut Node, phase: Phase) -> (result: ()) [
        requires old(m).generation < u64::MAX,
        ensures
            *final(m) == (Node {
                generation: (old(m).generation + 1) as u64,
                phase, pos: 0, ticket: None,
                effect: match old(m).effect {
                    Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                },
                ..*old(m)
            }),
            final(m).generation as int == old(m).generation as int + 1,
            final(m).phase == phase, final(m).pos == 0, final(m).ticket.is_none(),
            final(m).effect == match old(m).effect {
                Some(e) => Some(InFlight { cancelling: true, ..e }),
                None => None,
            },
            final(m).credit == old(m).credit, final(m).next_seq == old(m).next_seq,
            final(m).last_cmd == old(m).last_cmd, final(m).question == old(m).question,
            final(m).run == old(m).run, final(m).job == old(m).job,
            final(m).pool == old(m).pool, final(m).holds == old(m).holds,
            final(m).paused == old(m).paused, final(m).waiting_incident == old(m).waiting_incident,
    ] {
        if let Some(e) = m.effect.as_mut() {
            e.cancelling = true;
        }
        m.phase = phase;
        m.pos = 0;
        m.ticket = None;
        m.generation += 1;
    }
}

kernel_fn! {
/// Q30 / #771: releasing an operation preserves all allocation counters.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
fn abandon_operation(m: &mut Node) -> (result: ()) [
    ensures *final(m) == (Node {
        run: None, job: None, pool: false,
        effect: match old(m).effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || matches!(e.kind, EffectKind::StartRun(_)
                    | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
            None => None,
        },
        ..*old(m)
    }),
] {
    m.run = None;
    m.job = None;
    m.pool = false;
    if let Some(e) = m.effect.as_mut() {
        if e.kind.starts_operation() {
            e.cancelling = true;
        }
    }
}
}

kernel_fn! {
/// #771: successful completion resets failures and preserves allocation counters.
fn end_operation(m: &mut Node) -> (result: ()) [
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).run.is_none(), final(m).job.is_none(), !final(m).pool,
        final(m).run_failures == 0, final(m).run_incidents == 0,
        final(m).phase == old(m).phase, final(m).pos == old(m).pos,
        final(m).generation == old(m).generation, final(m).next_seq == old(m).next_seq,
        final(m).credit == old(m).credit, final(m).question == old(m).question,
        final(m).ticket == old(m).ticket,
        final(m).footprint == old(m).footprint,
        final(m).last_cmd == old(m).last_cmd,
        final(m).fx_failures == old(m).fx_failures,
        final(m).fx_incidents == old(m).fx_incidents,

] {
    abandon_operation(m);
    m.run_failures = 0;
    m.run_incidents = 0;
}
}

kernel_fn! {
/// Stop and replacement rows / #771: at most one generation is allocated.
fn enter_stopping(m: &mut Node, outcome: Outcome) -> (result: ()) [
    requires !(old(m).phase is Closed || old(m).phase is Stopping)
        ==> old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int == old(m).generation as int
            + if old(m).phase is Closed || old(m).phase is Stopping { 0int } else { 1int },
        final(m).phase == match old(m).phase {
            Phase::Closed(o) => Phase::Closed(o),
            Phase::Stopping(o) => Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }),
            _ => Phase::Stopping(outcome),
        },
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd,
        final(m).review_stop == old(m).review_stop,
        (old(m).phase is Closed || old(m).phase is Stopping) ==> *final(m) == (Node {
            phase: final(m).phase, ..*old(m)
        }),
        !(old(m).phase is Closed || old(m).phase is Stopping) ==>
            final(m).pos == 0 && final(m).ticket.is_none() && final(m).question.is_none()
                && !final(m).waiting_incident && !final(m).incident_needs_rob,
] {
    match m.phase {
        Phase::Closed(_) => {}
        Phase::Stopping(_) => {
            if outcome == Outcome::Merged {
                m.phase = Phase::Stopping(Outcome::Merged);
            }
        }
        _ => {
            m.question = None;
            m.waiting_incident = false;
            m.incident_needs_rob = false;
            set_phase(m, Phase::Stopping(outcome));
        }
    }
}
}

kernel_fn! {
    /// Q18 / #771: merge credit is allocated once, increasing earned by at most one.
    fn merge_credit(m: &mut Node) -> (result: ()) [
        requires old(m).credit.merge_credited || old(m).credit.earned < u32::MAX,
        ensures
            *final(m) == (Node { credit: Credit {
                earned: final(m).credit.earned, merge_credited: true, ..old(m).credit
            }, ..*old(m) }),
            final(m).credit.earned as int == old(m).credit.earned as int
                + if old(m).credit.merge_credited { 0int } else { 1int },
            final(m).credit.merge_credited,
            final(m).credit.start == old(m).credit.start,
            final(m).credit.added == old(m).credit.added,
            final(m).credit.spent == old(m).credit.spent,
            final(m).credit.ready == old(m).credit.ready,
            final(m).credit.withheld == old(m).credit.withheld,
            final(m).generation == old(m).generation, final(m).next_seq == old(m).next_seq,
            final(m).phase == old(m).phase, final(m).pos == old(m).pos,
            final(m).effect == old(m).effect, final(m).question == old(m).question,
        final(m).ticket == old(m).ticket,
            final(m).paused == old(m).paused,
            final(m).waiting_incident == old(m).waiting_incident,
    ] {
        if !m.credit.merge_credited {
            m.credit.earned += 1;
            m.credit.merge_credited = true;
        }
    }
}

kernel_fn! {
    fn close_node(m: &mut Node, i: usize, outcome: Outcome, out: &mut Outbox) -> (result: ()) [
    requires old(m).credit.merge_credited || old(m).credit.earned < u32::MAX,
        old(out).len + 2 <= OUTBOX,
    ensures final(m).phase == Phase::Closed(outcome), final(m).pos == 0,
        *final(m) == (Node { phase: Phase::Closed(outcome), pos: 0, ticket: None,
            credit: Credit { earned: final(m).credit.earned,
                merge_credited: final(m).credit.merge_credited, ..old(m).credit }, ..*old(m) }),
        final(m).ticket.is_none(),
        final(m).generation == old(m).generation, final(m).next_seq == old(m).next_seq,
        final(m).credit.earned as int == old(m).credit.earned as int
            + if outcome == Outcome::Merged && !old(m).credit.merge_credited { 1int } else { 0int },
        final(m).credit.merge_credited == (old(m).credit.merge_credited || outcome == Outcome::Merged),
        final(m).credit.start == old(m).credit.start, final(m).credit.added == old(m).credit.added,
        final(m).credit.spent == old(m).credit.spent,
        final(out).len <= old(out).len + 2,
    ] {
        m.phase = Phase::Closed(outcome);
        m.pos = 0;
        m.ticket = None;
        if outcome == Outcome::Merged {
            merge_credit(m);
        }
        notice(out, i, Notice::Summary(outcome));
        if outcome == Outcome::Replaced {
            notice(out, i, Notice::FileReplacement);
        }
    }
}

kernel_fn! {
/// Question rows / #771: one question and one generation, on the selected node.
fn ask_rob(w: &mut World, i: usize, kind: QKind, cont: Cont, cost: u8,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].generation < u64::MAX, old(out).len < OUTBOX,
    ensures final(w).next_question as int == old(w).next_question as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).nodes@ == old(w).nodes@.update(i as int, Node {
            holds: false,
            question: Some(Question { id: old(w).next_question, kind, cont, cost, clarified: false }),
            phase: Phase::AwaitingRob,
            generation: (old(w).nodes@[i as int].generation + 1) as u64,
            pos: 0, ticket: None,
            effect: match old(w).nodes@[i as int].effect {
                Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
            },
            ..old(w).nodes@[i as int]
        }),
        final(out).len == old(out).len + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
] {
    let id = w.next_question;
    w.next_question += 1;
    let m = &mut w.nodes[i];
    m.holds = false;
    m.question = Some(Question {
        id,
        kind,
        cont,
        cost,
        clarified: false,
    });
    set_phase(m, Phase::AwaitingRob);
    notice(out, i, Notice::Ask { question: id });
}
}

kernel_fn! {
/// Incident rows / #771: only a holding phase allocates a generation.
fn raise_incident(w: &mut World, i: usize, needs_rob: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        holding(old(w).nodes@[i as int].phase) ==>
            old(w).nodes@[i as int].generation < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int
            == old(w).nodes@[i as int].generation as int
                + if holding(old(w).nodes@[i as int].phase) { 1int } else { 0int },
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].waiting_incident,
        final(w).nodes@[i as int].incident_needs_rob == needs_rob,
        final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].run_failures == old(w).nodes@[i as int].run_failures,
        final(w).nodes@[i as int].run_incidents == old(w).nodes@[i as int].run_incidents,
        final(w).nodes@[i as int].fx_failures == old(w).nodes@[i as int].fx_failures,
        final(w).nodes@[i as int].fx_incidents == old(w).nodes@[i as int].fx_incidents,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 1,
] {
    let m = &mut w.nodes[i];
    m.waiting_incident = true;
    m.incident_needs_rob = needs_rob;
    m.ticket = None;
    // A leaf waiting on an incident gives back its slot and files.
    if let Some(r) = m.phase.resume() {
        m.holds = false;
        set_phase(m, Phase::Ready(r));
    }
    notice(out, i, Notice::Incident { needs_rob });
}
}

kernel_fn! {
/// #771: the world wrapper emits on exactly one selected node.
fn emit(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].next_seq < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].next_seq as int == old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].generation == old(w).nodes@[i as int].generation,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase,
        final(w).nodes@[i as int].pos == old(w).nodes@[i as int].pos,
        final(w).nodes@[i as int].fx_failures == old(w).nodes@[i as int].fx_failures,
        final(w).nodes@[i as int].fx_incidents == old(w).nodes@[i as int].fx_incidents,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 1,
] {
    emit_node(&mut w.nodes[i], i, kind, out);
}
}

kernel_fn! {
    /// #771: effect allocation increases next_seq by exactly one.
    fn emit_node(m: &mut Node, i: usize, kind: EffectKind, out: &mut Outbox) -> (result: ()) [
    requires old(m).next_seq < u64::MAX, old(out).len < OUTBOX,
    ensures *final(m) == (Node {
        next_seq: (old(m).next_seq + 1) as u64,
        effect: Some(InFlight { seq: old(m).next_seq, kind, cancelling: false }),
        worktree: old(m).worktree || kind == EffectKind::PrepareWorktree,
        pr_open: old(m).pr_open || kind == EffectKind::OpenPr,
        had_pr: old(m).had_pr || kind == EffectKind::OpenPr,
        ..*old(m)
    }),
    final(out).len == old(out).len + 1,
    ] {
        let seq = m.next_seq;
        m.next_seq += 1;
        m.effect = Some(InFlight {
            seq,
            kind,
            cancelling: false,
        });
        match kind {
            EffectKind::PrepareWorktree => m.worktree = true,
            EffectKind::OpenPr => {
                m.pr_open = true;
                m.had_pr = true;
            }
            _ => {}
        }
        let head = match kind {
            EffectKind::Push | EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::Enqueue => {
                m.head
            }
            _ => None,
        };
        out.push(Out::Effect {
            node: NodeId(i as u64),
            seq,
            kind,
            head,
        });
    }
}

// ---------------------------------------------------------------- replies

kernel_fn! {
/// Reply rows / #771: phase, schema and commit identity use the executable checks.
fn accepts(w: &World, n: &Node, reply: &Reply) -> (result: bool) [
    ensures result == match (n.phase, *reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) =>
            footprint::valid_footprint(*w, footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) =>
            footprint::valid_footprint(*w, files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        (Phase::Checking, Reply::ChecksDone { report, .. }) => report,
        (Phase::Reviewing, Reply::Approved { head }) =>
            n.head == Some(head) && n.checked == Some(head),
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges) => true,
        _ => false,
    },
] {
    match (n.phase, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => {
            true
        }
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) => w.has_footprint(*footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) => w.has_footprint(*files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        // No report means the tests never ran: an infrastructure failure.
        (Phase::Checking, Reply::ChecksDone { report, .. }) => *report,
        (Phase::Reviewing, Reply::Approved { head }) => {
            n.head == Some(*head) && n.checked == Some(*head)
        }
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. }) => true,
        (Phase::Rebasing, Reply::RebaseNeedsChanges) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Reply rows / #771: stale operations stutter, invalid replies use the bounded
/// failure handler, and successful replies use the actual transition bodies.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn on_reply(w: &mut World, i: usize, generation: u64, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start as int <= old(w).nodes@[i as int].credit.start as int + CREDIT_START_STEP_BUDGET,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] proof [reveal(reply_bounds); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_reply = *w }
    let n = &w.nodes[i];
    if n.phase.is_stopping() {
        // The run or job ended on its own, so teardown needn't stop it.
        let m = &mut w.nodes[i];
        if m.run == Some(generation) {
            m.run = None;
            m.pool = false;
        }
        if m.job == Some(generation) {
            m.job = None;
        }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        return;
    }
    if n.current() != Some(Item::Await) || (n.run != Some(generation) && n.job != Some(generation))
    {
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        return; // stale
    }
    // A reply that breaks its schema or names the wrong commit counts as a failed run.
    if !accepts(w, n, &reply) {
        run_failed(w, i, out);
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        return;
    }
    kernel_snapshot! { before_operation = *w }
    end_operation(&mut w.nodes[i]);
    kernel_proof! {
        footprint::valid_under_table_growth(before_operation@, *w, w.nodes@[i as int].footprint);
    }
    match reply {
        Reply::MakerNeedsFiles { files, fits } => {
            kernel_proof! { footprint::valid_under_table_growth(before_operation@, *w, files); }
            reply_files(w, i, files, fits, out);
        }
        _ => {
            reply_transition(w, i, reply, out);
            kernel_proof! {
                if let Reply::PlanLeaf { footprint, .. } = reply {
                    footprint::valid_under_table_growth(before_operation@, *w, footprint);
                }
                footprint::valid_under_table_growth(before_operation@, *w, before_operation@.nodes@[i as int].footprint);
            }
        }
    }
}
}

kernel_fn! {
/// File extension rows / #771: union only appends file storage; the transition
/// spends at most one credit and allocates one generation or question.
fn reply_files(w: &mut World, i: usize, files: Footprint, fits: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::valid_footprint(*old(w), files), footprint::ordered_table(*old(w)),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] proof [reveal(retry_bounds); reveal(footprint::ordered_table);] {
    if fits && !files.is_empty() && w.nodes[i].credit.can_spend(1) {
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        let m = &mut w.nodes[i];
        m.credit.spend(1);
        m.footprint = footprint;
        m.holds = false;
        set_phase(m, Phase::Ready(Resume::Making));
    } else if fits && !files.is_empty() && w.fail_fast {
        // Q54 stops retries, not credit decisions: Rob funds the extension.
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        w.nodes[i].footprint = footprint;
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(Resume::Making),
            1,
            out,
        );
    } else {
        out_of_room(w, i, ReviewStop::MakerNeedsFiles, out);
    }
    }
}

kernel_fn! {
/// Reply rows / #771: partition the unchanged match for bounded solver queries.
fn reply_transition(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] {
    match reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees
            | Reply::AuditDisagrees | Reply::ProtectedPaths => reply_ingest(w, i, reply, out),
        Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet => reply_plan(w, i, reply, out),
        _ => reply_work(w, i, reply, out),
    }
}
}

kernel_fn! {
/// Ingest reply rows / #771: initialize the same one-time deposit before audit.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn finish_ingest(w: &mut World, i: usize, size: u8, early_end: Option<EarlyEnd>)
    -> (result: ()) [
    requires i < old(w).nodes@.len(),
        node_headroom(old(w).nodes@[i as int]),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, 0, 0),
] proof [reveal(reply_bounds);] {
    let m = &mut w.nodes[i];
    if !m.credit.ready {
        m.credit.ready = true;
        m.credit.start = size as u32 + RESERVE;
    }
    m.early_end = early_end;
    set_phase(m, Phase::Auditing);
    w.withhold_dependency_credit(i);
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn reply_ingest(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::IngestDone { size } => {
            finish_ingest(w, i, size, None);
        }
        Reply::IngestEarlyEnd(e) => {
            finish_ingest(w, i, 1, Some(e));
        }
        Reply::AuditAgrees => match w.nodes[i].early_end {
            Some(e) => enter_stopping(&mut w.nodes[i], e.outcome()),
            None => {
                notice(out, i, Notice::MeasuresPosted);
                set_phase(&mut w.nodes[i], Phase::Unplanned);
            }
        },
        Reply::AuditDisagrees => {
            let m = &mut w.nodes[i];
            m.audit_rounds = m.audit_rounds.saturating_add(1);
            if m.audit_rounds < 2 {
                // Ingest revises once with the audit's objections.
                set_phase(m, Phase::Ingesting);
            } else {
                let cont = match m.early_end {
                    Some(e) => Cont::Stop(e.outcome()),
                    None => Cont::Unplanned,
                };
                ask_rob(w, i, QKind::AuditDisagreement, cont, 0, out);
            }
        }
        Reply::ProtectedPaths => enter_stopping(&mut w.nodes[i], Outcome::NotActionable),
        _ => {}
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
fn reply_plan(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::PlanLeaf { footprint, fits } => {
            let m = &mut w.nodes[i];
            let must_split = matches!(m.phase, Phase::Planning { must_split: true });
            if fits && !footprint.is_empty() && !must_split {
                m.footprint = footprint;
                m.attempt = 1;
                m.review_restarted = false;
                m.round = 0;
                m.plan_retries = 0;
                set_phase(m, Phase::Ready(Resume::Making));
            } else {
                plan_retry(w, i, false, out);
            }
        }
        Reply::PlanSplit => ask_rob(w, i, QKind::NeedsSplitting, Cont::Nowhere, 0, out),
        Reply::PlanCantMeet => plan_retry(w, i, true, out),
        _ => {}
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn reply_work(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::MakerDone { head } => {
            let m = &mut w.nodes[i];
            m.head = Some(head);
            m.checked = None;
            set_phase(m, Phase::Checking);
        }
        Reply::MakerCantMeet => next_rung(w, i, ReviewStop::MakerCantMeet, out),
        Reply::MakerOutOfRoom => out_of_room(w, i, ReviewStop::MakerOutOfRoom, out),
        Reply::ChecksDone {
            pass, in_footprint, ..
        } => {
            if pass && in_footprint {
                let m = &mut w.nodes[i];
                m.checked = m.head;
                set_phase(m, Phase::Reviewing);
            } else {
                failed_round(w, i, ReviewStop::ChecksFailed, out);
            }
        }
        Reply::Approved { head } => {
            let m = &mut w.nodes[i];
            m.approved = Some(head);
            set_phase(m, Phase::Publishing);
        }
        Reply::ChangesRequested => failed_round(w, i, ReviewStop::ChangesRequested, out),
        Reply::ReviewerOutOfRoom => {
            if w.nodes[i].review_restarted {
                out_of_room(w, i, ReviewStop::ReviewerOutOfRoom, out);
            } else {
                w.nodes[i].review_restarted = true;
                recheck(w, i, Phase::Reviewing, out);
            }
        }
        _ => reply_rebase(w, i, reply, out),
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
fn reply_rebase(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::RebaseClean { head } => {
            let m = &mut w.nodes[i];
            if m.head != Some(head) {
                m.review_restarted = false;
            }
            m.head = Some(head);
            m.checked = None;
            recheck(w, i, Phase::Checking, out);
        }
        Reply::RebaseNeedsChanges => {
            let m = &mut w.nodes[i];
            if m.credit.can_spend(1) {
                m.credit.spend(1);
                set_phase(m, Phase::Making);
            } else {
                ask_rob(
                    w,
                    i,
                    QKind::OutOfCredit,
                    Cont::Ready(Resume::Making),
                    1,
                    out,
                );
            }
        }
        _ => {}
    }
}
}

kernel_fn! {
/// Failed-run rows / #771: a retry and incident allocate at most two generations.
fn run_failed(w: &mut World, i: usize, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].run_failures <= 1,
        old(w).nodes@[i as int].run_incidents <= 2,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].run_failures <= 1,
        final(w).nodes@[i as int].run_incidents <= 2,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let m = &mut w.nodes[i];
    abandon_operation(m);
    m.pos = operation_start(m.phase);
    m.generation += 1;
    m.run_failures += 1;
    if m.run_failures >= 2 {
        // Every second failure raises an incident. Only Rob can clear the second one.
        m.run_failures = 0;
        m.run_incidents = (m.run_incidents + 1).min(2);
        let needs_rob = m.run_incidents >= 2;
        raise_incident(w, i, needs_rob, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: stopping for review spends no credit and allocates at most one generation.
/// Under `fail_fast`, a step that would retry the work stops for TL3 review.
/// It asks Rob nothing; the summary offers `restart`, as for any stop (Q49).
fn stop_for_review(m: &mut Node, step: ReviewStop) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd, final(m).review_stop == Some(step),
] {
    m.review_stop = Some(step);
    enter_stopping(m, Outcome::Stopped);
}
}

kernel_fn! {
/// Failed-round rows / #771: the free fix and paid ladder share one-step bounds.
fn failed_round(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(retry_bounds);] {
    let m = &mut w.nodes[i];
    if m.round == 0 {
        // The fix round resumes the maker's session. It's free.
        m.round = 1;
        set_phase(m, Phase::Making);
    } else {
        next_rung(w, i, step, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: the ladder after a failed fix round costs at most one credit.
/// It tries a bigger budget, another model, one re-plan, then asks Rob.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn next_rung(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
    } else if m.attempt < 3 {
        m.attempt += 1;
        m.review_restarted = false;
        m.round = 0;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            set_phase(m, Phase::Making);
        } else {
            ask_rob(
                w,
                i,
                QKind::OutOfCredit,
                Cont::Ready(Resume::Making),
                1,
                out,
            );
        }
    } else if !m.replanned {
        m.replanned = true;
        m.holds = false;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split: false });
            notice(out, i, Notice::Note(NoteKind::Replanned));
        } else {
            ask_rob(
                w,
                i,
                QKind::OutOfCredit,
                Cont::Planning { must_split: false },
                1,
                out,
            );
        }
    } else {
        ask_rob(w, i, QKind::CantMeet, Cont::Ready(Resume::Making), 1, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: exhaustion either stops, pays once to re-plan, or asks Rob.
fn out_of_room(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
        return;
    }
    m.holds = false;
    if m.credit.can_spend(1) {
        m.credit.spend(1);
        m.plan_retries = 0;
        set_phase(m, Phase::Planning { must_split: true });
    } else {
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Planning { must_split: true },
            1,
            out,
        );
    }
}
}

kernel_fn! {
/// Q54 / #771: planner retries saturate their local retry count.
/// Under `fail_fast`, `cant_meet` stops for review. A plan that does not fit
/// keeps the existing retry rule; the live worker reports broken rules as `cant_meet`.
fn plan_retry(w: &mut World, i: usize, cant_meet: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    m.plan_retries = m.plan_retries.saturating_add(1);
    let phase = m.phase;
    if fail_fast && cant_meet {
        stop_for_review(m, ReviewStop::PlanCantMeet);
    } else if m.plan_retries < 2 {
        // Re-run once, with the reason or the next model on the planner's ladder.
        set_phase(m, phase);
    } else {
        m.plan_retries = 0;
        let must_split = matches!(phase, Phase::Planning { must_split: true });
        ask_rob(w, i, QKind::CantMeet, Cont::Planning { must_split }, 1, out);
    }
}
}

kernel_fn! {
/// Recheck rows / #771: a new head or reviewer uses the same free allowance.
fn recheck(w: &mut World, i: usize, target: Phase,
    out: &mut Outbox) -> (result: ()) [
    requires !(target is Closed), i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
] proof [reveal(retry_bounds);] {
    let m = &mut w.nodes[i];
    m.rechecks = m.rechecks.saturating_add(1);
    let free = m.rechecks <= FREE_RECHECKS;
    if free || m.credit.can_spend(1) {
        if !free {
            m.credit.spend(1);
        }
        if !target.holding() {
            m.holds = false;
        }
        set_phase(m, target);
    } else {
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(if target == Phase::Reviewing {
                Resume::Reviewing
            } else {
                Resume::Checking
            }),
            1,
            out,
        );
    }
}
}

// ---------------------------------------------------------------- acknowledgements

kernel_fn! {
/// Acknowledgement rows / #771: stale delivery keys stutter; retries stay bounded.
fn on_ack(w: &mut World, i: usize, seq: u64, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed && old(w).nodes@[i as int].effect.is_none()
            ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) && stopping_effect_fits(old(w).nodes@[i as int])
            ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq as int <= old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
] {
    let Some(e) = w.nodes[i].effect else { return };
    if e.seq != seq {
        return;
    }
    w.nodes[i].effect = None;
    if e.cancelling {
        return;
    }
    match ack {
        Ack::Confirmed => confirmed(&mut w.nodes[i], e.kind),
        Ack::Retryable => delivery_failed(w, i, e.kind, out),
        Ack::Permanent => effect_failed(w, i, e.kind, out),
        Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PublicationBody);
        }
        Ack::BodyPolicyRefused => effect_failed(w, i, e.kind, out),
    }
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: at most one adoption generation is allocated.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn confirmed(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m))
            && (old(m).phase is Stopping ==> current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).generation >= old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).fx_failures == 0, final(m).fx_incidents == 0,
] {
    m.fx_failures = 0;
    m.fx_incidents = 0;
    match kind {
        EffectKind::StopRun => {
            m.run = None;
            m.pool = false;
        }
        EffectKind::WithdrawJob => m.job = None,
        EffectKind::ClosePr => m.pr_open = false,
        EffectKind::RemoveWorktree => m.worktree = false,
        _ => {}
    }
    if !kind.starts_operation() {
        if let Some(Item::Do(k) | Item::Teardown(k)) = m.current() {
            if k == kind {
                if kind == EffectKind::PrepareWorktree && !m.adopted {
                    if let Some(head) = m.proposed {
                        m.adopted = true;
                        m.head = Some(head);
                        m.checked = None;
                        set_phase(m, Phase::Checking);
                        return;
                    }
                }
                m.pos += 1;
            }
        }
    }
}
}

kernel_fn! {
/// Failed-delivery rows / #771: at most two generations or one retried effect.
fn delivery_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].next_seq < u64::MAX,
        old(w).nodes@[i as int].fx_failures <= 1,
        old(w).nodes@[i as int].fx_incidents <= 2,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq as int <= old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].fx_failures <= 1,
        final(w).nodes@[i as int].fx_incidents <= 2,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let m = &mut w.nodes[i];
    m.fx_failures += 1;
    if m.fx_failures < 2 {
        emit(w, i, kind, out);
        return;
    }
    m.fx_failures = 0;
    m.fx_incidents = (m.fx_incidents + 1).min(2);
    let needs_rob = m.fx_incidents >= 2;
    if kind.starts_operation() {
        abandon_operation(m);
        m.pos = operation_start(m.phase);
        m.generation += 1;
    }
    raise_incident(w, i, needs_rob, out);
}
}

kernel_fn! {
/// Permanent-effect-failure rows / #771: exactly one question, no credit change.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn effect_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int == old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 2,
] {
    notice(out, i, Notice::Incident { needs_rob: true });
    if w.nodes[i].phase.is_stopping() {
        // Cleanup stays put, keeping what it holds, until Rob retries or marks the step handled.
        let id = w.next_question;
        w.next_question += 1;
        w.nodes[i].question = Some(Question {
            id,
            kind: QKind::CleanupFailed,
            cont: Cont::Nowhere,
            cost: 0,
            clarified: false,
        });
        notice(out, i, Notice::Ask { question: id });
        return;
    }
    let m = &mut w.nodes[i];
    if kind.starts_operation() {
        abandon_operation(m);
    }
    let cont = match m.phase {
        Phase::Ingesting => Cont::Ingesting,
        Phase::Auditing => Cont::Auditing,
        Phase::Planning { must_split } => Cont::Planning { must_split },
        phase => match phase.resume() {
            Some(r) => Cont::Ready(r),
            None => Cont::Unplanned,
        },
    };
    ask_rob(w, i, QKind::EffectFailed, cont, 0, out);
}
}

// ---------------------------------------------------------------- facts

kernel_fn! {
/// Fact rows / #771: compose declaration updates, merge credit and the remaining
/// fact handlers. No fact assigns a command ID, effect sequence or ticket.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
fn on_fact(w: &mut World, i: usize, fact: Fact, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        dependencies::edges_indexable(*old(w)), old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned as int + merge_capacity(final(w).nodes@[i as int])
            <= old(w).nodes@[i as int].credit.earned as int + 1,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(out).len <= old(out).len + 4,
] {
    match fact {
        Fact::DependencyRemove { kind, number } => w.remove_dependency(i, kind, number),
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        Fact::PrMerged {
            head,
            checks_passed,
        } => pr_merged(w, i, head, checks_passed, out),
        _ => fact_transition(w, i, fact, out),
    }
}
}

kernel_fn! {
/// Fact rows / #771: non-dependency, non-merge facts allocate at most one
/// generation, one question and one paid credit, and emit at most two notices.
#[allow(clippy::collapsible_if)]
fn fact_transition(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(fact is DependencyRemove || fact is Dependency || fact is PrMerged),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
] proof [reveal(retry_bounds);] {
    match fact {
        Fact::DependencyRemove { .. } | Fact::Dependency { .. } | Fact::PrMerged { .. } => {}
        Fact::Priority(priority) => {
            if !w.nodes[i].phase.is_closed() {
                w.nodes[i].priority = priority;
            }
        }
        Fact::PrClosedByPerson => {
            let m = &mut w.nodes[i];
            if !m.pr_open {
                return;
            }
            m.pr_open = false;
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                notice(out, i, Notice::Note(NoteKind::PrClosed));
                enter_stopping(m, Outcome::Stopped);
            }
        }
        Fact::PrDequeuedByPerson => {
            let m = &mut w.nodes[i];
            if m.phase != Phase::Queued {
                return;
            }
            // Treated as Rob pausing it: the PR waits until he resumes.
            m.holds = false;
            m.paused = true;
            set_phase(m, Phase::Ready(Resume::Publishing));
            notice(out, i, Notice::Note(NoteKind::PrDequeued));
        }
        Fact::PushByOther { head } => {
            let m = &w.nodes[i];
            if !m.had_pr {
                return;
            }
            notice(out, i, Notice::Note(NoteKind::OutsidePush));
            let published = matches!(
                m.phase,
                Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing)
            );
            if published {
                let m = &mut w.nodes[i];
                if m.head != Some(head) {
                    m.review_restarted = false;
                }
                m.head = Some(head);
                m.checked = None;
                recheck(w, i, Phase::Ready(Resume::Checking), out);
            }
        }
        Fact::CiFailed { infra } => {
            let m = &mut w.nodes[i];
            let before_enqueue =
                m.phase == Phase::Publishing && m.current() == Some(Item::Do(EffectKind::Enqueue));
            if m.phase != Phase::Queued && !before_enqueue {
                return;
            }
            if before_enqueue {
                if let Some(effect) = m.effect.as_mut() {
                    effect.cancelling = true;
                }
            }
            if !infra {
                failed_round(w, i, ReviewStop::CiFailed, out);
                return;
            }
            let m = &mut w.nodes[i];
            m.run_failures += 1;
            if m.run_failures < 2 {
                set_phase(m, Phase::Publishing); // enqueue again
            } else {
                m.run_failures = 0;
                m.run_incidents = (m.run_incidents + 1).min(2);
                let needs_rob = m.run_incidents >= 2;
                raise_incident(w, i, needs_rob, out);
            }
        }
        Fact::Conflict => {
            let m = &mut w.nodes[i];
            if matches!(m.phase, Phase::Publishing | Phase::Queued) {
                set_phase(m, Phase::Rebasing);
            }
        }
        Fact::UpstreamStalled => {
            let m = &mut w.nodes[i];
            if m.phase != Phase::Queued || m.waiting_incident {
                return;
            }
            m.run_incidents = (m.run_incidents + 1).min(2);
            let needs_rob = m.run_incidents >= 2;
            raise_incident(w, i, needs_rob, out);
        }
        Fact::IncidentCleared { by_rob } => {
            let m = &mut w.nodes[i];
            if m.waiting_incident && (by_rob || !m.incident_needs_rob) {
                m.waiting_incident = false;
                m.incident_needs_rob = false;
            }
        }
    }
}
}

kernel_fn! {
/// Merge fact rows / Q18 / #771: a late merge earns credit once; other merges
/// enter teardown. Even an unapproved late merge emits at most four notices.
fn pr_merged(w: &mut World, i: usize, head: HeadId, checks_passed: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 4 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned as int <= old(w).nodes@[i as int].credit.earned as int + 1,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent == old(w).nodes@[i as int].credit.spent,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 4,
        final(w).nodes@[i as int].credit.earned as int + merge_capacity(final(w).nodes@[i as int])
            <= old(w).nodes@[i as int].credit.earned as int + 1,

] {
    let m = &mut w.nodes[i];
    if !m.had_pr || m.merged.is_some() {
        return;
    }
    let approved = m.approved == Some(head) && checks_passed;
    match m.phase {
        Phase::Closed(o) => {
            if !matches!(o, Outcome::Stopped | Outcome::Replaced) {
                return;
            }
            // A late merge corrects the record and earns its credit once.
            m.merged = Some(head);
            m.phase = Phase::Closed(Outcome::Merged);
            merge_credit(m);
            notice(out, i, Notice::Note(NoteKind::LateMerge));
            notice(out, i, Notice::Incident { needs_rob: false });
        }
        _ => {
            m.merged = Some(head);
            m.pr_open = false;
            enter_stopping(m, Outcome::Merged);
        }
    }
    if !approved {
        notice(out, i, Notice::Incident { needs_rob: false });
        notice(out, i, Notice::Note(NoteKind::UnapprovedMerge));
    }
}
}

// ---------------------------------------------------------------- commands

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
fn on_command(w: &mut World, i: usize, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == if id > old(w).nodes@[i as int].last_cmd {
            id
        } else { old(w).nodes@[i as int].last_cmd },
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    if id <= w.nodes[i].last_cmd {
        return; // a repeated command
    }
    w.nodes[i].last_cmd = id;
    match command {
        Command::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
        Command::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
        Command::Pause => {
            let m = &mut w.nodes[i];
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                m.paused = true;
            }
        }
        Command::Resume => w.nodes[i].paused = false,
        Command::Answer { question, answer } => on_answer(w, i, question, answer, out),
    }
}
}

kernel_fn! {
/// Question answer rows / #771: cleanup, continuation and funding permissions.
fn allowed(kind: QKind, answer: Answer) -> (result: bool) [
    ensures result == match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    },
] {
    match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
fn on_answer(w: &mut World, i: usize, id: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let Some(q) = w.nodes[i].question else { return };
    if q.id != id {
        return; // an answer to a question that is no longer open
    }
    match w.nodes[i].phase {
        Phase::Stopping(_) => match answer {
            Answer::Retry => w.nodes[i].question = None,
            Answer::MarkHandled => {
                let m = &mut w.nodes[i];
                m.question = None;
                mark_handled(m);
            }
            _ => {}
        },
        Phase::AwaitingRob => {
            if answer == Answer::Unclear {
                // One clarifying question. After that the question just stays open.
                if !q.clarified {
                    let next = w.next_question;
                    w.next_question += 1;
                    w.nodes[i].question = Some(Question {
                        id: next,
                        clarified: true,
                        ..q
                    });
                    notice(out, i, Notice::Ask { question: next });
                }
                return;
            }
            if !allowed(q.kind, answer) {
                return;
            }
            match answer {
                Answer::Continue => go_on(w, i, q, 0, out),
                Answer::TryAgain => {
                    let m = &mut w.nodes[i];
                    m.attempt = 1;
                    m.review_restarted = false;
                    m.round = 0;
                    m.replanned = false;
                    go_on(
                        w,
                        i,
                        Question {
                            cost: q.cost.max(1),
                            ..q
                        },
                        0,
                        out,
                    );
                }
                Answer::AddCredit(k) => {
                    go_on(w, i, q, k, out);
                }
                Answer::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
                Answer::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
                _ => {}
            }
        }
        _ => {}
    }
}
}

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase.
fn go_on(w: &mut World, i: usize, q: Question, deposit: u8,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX, q.cost <= 1,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).nodes@[i as int].credit.start as int
            + old(w).nodes@[i as int].credit.earned as int
            + old(w).nodes@[i as int].credit.added as int
                >= old(w).nodes@[i as int].credit.spent as int,
        old(w).nodes@[i as int].credit.withheld ==> q.cost == 0,
        old(w).nodes@[i as int].credit.added as int + 255 <= u32::MAX as int,
        old(w).nodes@[i as int].credit.spent < u32::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int].question.is_none(),
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let m = &mut w.nodes[i];
    m.credit.fund_and_pay(deposit, q.cost);
    m.question = None;
    match q.cont {
        Cont::Ingesting => set_phase(m, Phase::Ingesting),
        Cont::Auditing => set_phase(m, Phase::Auditing),
        Cont::Unplanned => set_phase(m, Phase::Unplanned),
        Cont::Planning { must_split } => {
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split });
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
        }
        Cont::Ready(r) => set_phase(m, Phase::Ready(r)),
        Cont::Stop(o) => enter_stopping(m, o),
        Cont::Nowhere => enter_stopping(m, Outcome::Stopped),
    }
}
}

kernel_fn! {
/// Cleanup question row / #771: handling cleanup allocates no monotone counter.
fn mark_handled(m: &mut Node) -> (result: ()) [
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).phase == old(m).phase,
        final(m).last_cmd == old(m).last_cmd,
] {
    if let Some(Item::Teardown(kind)) = m.current() {
        match kind {
            EffectKind::StopRun => {
                m.run = None;
                m.pool = false;
            }
            EffectKind::WithdrawJob => m.job = None,
            EffectKind::ClosePr => m.pr_open = false,
            EffectKind::RemoveWorktree => m.worktree = false,
            _ => {}
        }
        m.pos += 1;
    }
}
}

// ---------------------------------------------------------------- settle

#[cfg(verus_keep_ghost)]
verus! {
/// #771: the actual readiness check, expressed without executing its guards.
spec fn ready_checked(n: Node, r: Resume) -> bool {
    1 <= n.attempt <= 3 && n.round <= 1 && !footprint::footprint_empty(n.footprint)
        && n.credit.ready && match r {
            Resume::Making => true,
            Resume::Checking | Resume::Rebasing => n.head.is_some(),
            Resume::Reviewing => n.head.is_some() && n.checked == n.head,
            Resume::Publishing => match n.head {
                Some(h) => n.approved == Some(h) && n.checked == Some(h), None => false,
            },
        }
}

/// #771: continuations accepted by the production AwaitingRob check.
spec fn question_fits(q: Question) -> bool {
    match (q.kind, q.cont) {
        (QKind::AuditDisagreement, Cont::Unplanned) => true,
        (QKind::AuditDisagreement, Cont::Stop(o)) => matches!(o,
            Outcome::Duplicate | Outcome::AlreadyDone | Outcome::NotActionable),
        (QKind::NeedsSplitting, Cont::Nowhere) => true,
        (QKind::CantMeet, Cont::Planning { .. } | Cont::Ready(Resume::Making)) => true,
        (QKind::OutOfCredit, Cont::Planning { .. } | Cont::Ready(Resume::Making)
            | Cont::Ready(Resume::Checking) | Cont::Ready(Resume::Reviewing)) => true,
        (QKind::EffectFailed, Cont::Ingesting | Cont::Auditing | Cont::Unplanned
            | Cont::Planning { .. } | Cont::Ready(_)) => true,
        _ => false,
    }
}

/// #771: the existing per-node consistency checks used by progress. This is
/// proved as a postcondition of check_node on the real Node and World types.
spec fn node_checked(w: World, n: Node) -> bool {
    n.repo.0 < w.gates@.len() && footprint::valid_footprint(w, n.footprint)
        && arithmetic_consistent(n) && settle_shape(n) && stopping_effect_fits(n)
        && (n.phase is Closed ==> n.pos == 0 && n.effect.is_none() && n.run.is_none()
            && n.job.is_none() && !n.pool && !n.holds && n.ticket.is_none()
            && n.question.is_none() && !n.waiting_incident && !n.pr_open && !n.worktree)
        && (!(n.phase is Closed) ==> n.pos <= items(n.phase).len()
            && (n.pos == items(n.phase).len() ==> n.phase == Phase::Publishing || n.phase is Stopping))
        && (holding(n.phase) ==> n.holds)
        && (n.holds ==> holding(n.phase) || (n.phase is Stopping && n.pos <= RELEASE_STEP))
        && (n.pool ==> n.run.is_some() && n.phase != Phase::Rebasing)
        && (match n.run {
            Some(g) => (current_item(n) == Some(Item::Await) && g == n.generation
                && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }
                    | Phase::Making | Phase::Reviewing | Phase::Rebasing))
                || (n.phase is Stopping && n.pos == 0),
            None => true,
        })
        && (match n.job {
            Some(g) => (n.phase == Phase::Checking && current_item(n) == Some(Item::Await) && g == n.generation)
                || (n.phase is Stopping && n.pos <= 1),
            None => true,
        })
        && (current_item(n) == Some(Item::Await) ==> if n.phase == Phase::Checking {
            n.job == Some(n.generation)
        } else { n.run == Some(n.generation) })
        && (match n.effect {
            None => true,
            Some(e) => e.seq < n.next_seq && (e.cancelling || if
                matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
                current_item(n) == Some(Item::Await)
            } else { current_item(n) == Some(Item::Do(e.kind)) || current_item(n) == Some(Item::Teardown(e.kind)) }),
        })
        && (match n.ticket { None => true, Some(t) => t < w.next_ticket
            && !n.waiting_incident && n.effect.is_none() && !n.pool
            && matches!(current_item(n), Some(Item::Slot) | Some(Item::Start(_))) })
        && (n.waiting_incident ==> !(n.phase is Closed) && !holding(n.phase)
            && n.ticket.is_none() && (match n.effect { None => true, Some(e) => e.cancelling })
            && (!(n.phase is Stopping) ==> n.run.is_none() && n.job.is_none() && !n.pool))
        && (match n.question {
            None => n.phase != Phase::AwaitingRob,
            Some(q) => q.id < w.next_question && (match n.effect { None => true, Some(e) => e.cancelling })
                && match n.phase {
                    Phase::AwaitingRob => question_fits(q) && n.run.is_none() && n.job.is_none() && !n.pool,
                    Phase::Stopping(_) => q.kind == QKind::CleanupFailed && matches!(current_item(n), Some(Item::Teardown(_))),
                    _ => false,
                },
        })
        && (match n.phase { Phase::Ready(r) => ready_checked(n, r), _ => true })
        && (match n.phase {
            Phase::Making => ready_checked(n, Resume::Making),
            Phase::Checking => ready_checked(n, Resume::Checking),
            Phase::Reviewing => ready_checked(n, Resume::Reviewing),
            Phase::Publishing | Phase::Queued => ready_checked(n, Resume::Publishing),
            Phase::Rebasing => ready_checked(n, Resume::Rebasing),
            _ => true,
        })
}

/// #771: progress facts obtained by running the production consistency checker.
pub closed spec fn world_checked(w: World) -> bool {
    w.gates@.len() > 0 && w.pool_capacity > 0
        && (forall|j: int| 0 <= j < w.gates@.len() ==> w.gates@[j].w > 0)
        && (forall|j: int| 0 <= j < w.nodes@.len() ==> node_checked(w, w.nodes@[j]))
        && dependencies::dependencies_checked(w)
}

/// Q30/Q39 / #771: obstruction proof needs no sufficient internal move. The
/// real is_settled and settle bodies both establish this predicate.
pub closed spec fn settled_world(w: World) -> bool {
    forall|i: int| 0 <= i < w.nodes@.len() ==> !movable(w, i)
}

/// #771: the production checker and enforced reserves supply the step scope.
proof fn checked_reserves(w: World)
    requires world_checked(w), world_headroom(w), footprint::ordered_table(w),
    ensures step_scope(w), settle_ready(w),
{
    reveal(world_checked);
    reveal(world_headroom);
    reveal(step_scope);
    settle_reserve_totals(w.nodes@);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies
        w.nodes@[j].generation as int + generation_capacity(w.nodes@[j]) <= u64::MAX as int
            && w.nodes@[j].next_seq as int + effect_capacity(w.nodes@[j]) <= u64::MAX as int
            && w.nodes@[j].credit.earned as int + merge_capacity(w.nodes@[j]) <= u32::MAX as int
    by { local_capacities(w.nodes@[j]); }
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    reveal(open_prefix);
}

/// #771: intermediate source-level safety scope. These consistency clauses
/// must be connected to the real checker before the final progress theorem.
pub closed spec fn step_scope(w: World) -> bool {
    world_headroom(w) && footprint::ordered_table(w) && dependencies::edges_indexable(w)
        && forall|j: int| 0 <= j < w.nodes@.len() ==>
            arithmetic_consistent(w.nodes@[j]) && settle_shape(w.nodes@[j])
                && stopping_effect_fits(w.nodes@[j])
                && (w.nodes@[j].phase is Closed ==> w.nodes@[j].effect.is_none())
                && footprint::valid_footprint(w, w.nodes@[j].footprint)
}

/// #771: proven per-step K values for the executable kernel-owned ledgers.
/// New nodes begin at zero before settle and fit the same constant budgets.
pub closed spec fn step_budget(before: World, after: World) -> bool {
    before.nodes@.len() <= after.nodes@.len() <= before.nodes@.len() + 1
        && after.next_question as int <= before.next_question as int + QUESTION_STEP_BUDGET
        && after.next_ticket as int <= before.next_ticket as int + 2 * (before.nodes@.len() + 1)
        && forall|j: int| 0 <= j < before.nodes@.len() ==>
            after.nodes@[j].generation as int <= before.nodes@[j].generation as int + GENERATION_STEP_BUDGET
                && after.nodes@[j].next_seq as int <= before.nodes@[j].next_seq as int + EFFECT_STEP_BUDGET
                && after.nodes@[j].credit.start as int <= before.nodes@[j].credit.start as int + CREDIT_START_STEP_BUDGET
                && after.nodes@[j].credit.earned as int <= before.nodes@[j].credit.earned as int + CREDIT_EARNED_STEP_BUDGET
                && after.nodes@[j].credit.added as int <= before.nodes@[j].credit.added as int + CREDIT_ADDED_STEP_BUDGET
                && after.nodes@[j].credit.spent as int <= before.nodes@[j].credit.spent as int + CREDIT_SPENT_STEP_BUDGET
}

/// #771: reserve for the remaining local settle chains, with mathematical sums.
/// #771: check_node requires every live stopping effect to fit its teardown.
spec fn stopping_effect_fits(n: Node) -> bool {
    n.phase is Stopping ==> match n.effect {
        Some(e) => e.cancelling || current_item(n) == Some(Item::Teardown(e.kind)),
        None => true,
    }
}

spec fn settle_ready(w: World) -> bool {
    scheduling_safe(w) && w.nodes@.len() <= u64::MAX as int
        && (forall|j: int| 0 <= j < w.nodes@.len() ==> settle_shape(w.nodes@[j])
            && w.nodes@[j].generation as int + generation_capacity(w.nodes@[j]) <= u64::MAX as int
            && w.nodes@[j].next_seq as int + effect_capacity(w.nodes@[j]) <= u64::MAX as int
            && w.nodes@[j].credit.earned as int + merge_capacity(w.nodes@[j]) <= u32::MAX as int)
        && w.next_ticket as int + potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| ticket_capacity(n)) <= u64::MAX as int
        && potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int
}

/// Q30/Q39 / #771: sufficient scheduling conditions for an actual internal move.
/// Pool obstruction is discharged first; only then can a slot obstruction be
/// reduced to the oldest Ready waiter. No fairness premise is used.
spec fn movable(w: World, i: int) -> bool {
    let node = w.nodes@[i];
    let configured = node.repo.0 < w.gates@.len() && w.gates@[node.repo.0 as int].w > 0;
    advance_enabled(node, AdvanceContext {
        node: i as usize,
        admit: configured && dependencies::delivered(w.dependencies@, NodeId(i as u64))
            && slot_waiters(w.nodes@, w.nodes@.len(), node.repo) == 0,
        pool: w.pool_capacity > 0 && pool_users(w.nodes@, w.nodes@.len()) == 0
            && oldest_waiter(w, i),
        slot: configured && oldest_waiter(w, i)
            && (forall|j: int| 0 <= j < w.nodes@.len() && j != i ==> !w.nodes@[j].holds),
    })
}

/// #771: conserved allocations and frames across the real settle loop.
spec fn settle_chain(before: World, after: World) -> bool {
    after.nodes@.len() == before.nodes@.len()
        && after.next_question == before.next_question && after.gates@ == before.gates@
        && after.pool_capacity == before.pool_capacity && after.fail_fast == before.fail_fast
        && after.file_sets@ == before.file_sets@ && after.dependencies@ == before.dependencies@
        && node_total(after.nodes@) <= node_total(before.nodes@)
        && after.next_ticket as int + potential_prefix(after.nodes@, after.nodes@.len(), |n: Node| ticket_capacity(n))
            <= before.next_ticket as int + potential_prefix(before.nodes@, before.nodes@.len(), |n: Node| ticket_capacity(n))
        && forall|j: int| 0 <= j < before.nodes@.len() ==>
            after.nodes@[j].generation as int + generation_capacity(after.nodes@[j])
                <= before.nodes@[j].generation as int + generation_capacity(before.nodes@[j])
            && after.nodes@[j].next_seq as int + effect_capacity(after.nodes@[j])
                <= before.nodes@[j].next_seq as int + effect_capacity(before.nodes@[j])
            && after.nodes@[j].credit.earned as int + merge_capacity(after.nodes@[j])
                <= before.nodes@[j].credit.earned as int + merge_capacity(before.nodes@[j])
            && after.nodes@[j].credit.start == before.nodes@[j].credit.start
            && after.nodes@[j].credit.added == before.nodes@[j].credit.added
            && after.nodes@[j].credit.spent == before.nodes@[j].credit.spent
}
}

kernel_fn! {
/// Takes passes over the nodes until one changes nothing, and returns how many
/// passes changed something. Each internal transition moves a node along a
/// finite chain, so the loop ends. Kani's unwinding check proves it within the
/// bounds.
fn settle(w: &mut World, out: &mut Vec<Out>, mut trace: Option<&mut Vec<World>>)
    -> (result: u32) [
    requires settle_ready(*old(w)),
    ensures settle_ready(*final(w)), settle_chain(*old(w), *final(w)),
        forall|i: int| 0 <= i < final(w).nodes@.len() ==> !movable(*final(w), i),
        result as int <= potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
] {
    kernel_snapshot! { baseline = *w }
    let mut passes: u32 = 0;
    kernel_loop! { [
        invariant baseline@ == *old(w), settle_ready(*w), settle_chain(baseline@, *w),
            passes as int + potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n))
                <= potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)),
            potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int,
        decreases potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)),
    ] {
        kernel_snapshot! { pass_height = potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) }
        let mut changed = false;
        let mut i = 0;
        kernel_while! { (i < w.nodes.len()) [
            invariant i <= w.nodes.len(), baseline@ == *old(w),
                settle_ready(*w), settle_chain(baseline@, *w),
                passes as int + pass_height@ <= potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)),
                potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int,
                potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) <= pass_height@,
                changed ==> potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) < pass_height@,
                !changed ==> forall|j: int| 0 <= j < i ==> !movable(*w, j),
            decreases w.nodes.len() - i,
        ] {
            let mut outbox = Outbox::new();
            kernel_proof! {
                reveal(empty_outbox);
                potential_member(w.nodes@, i as int, w.nodes@.len(), |n: Node| ticket_capacity(n));
            }
            kernel_snapshot! { before_node = *w }
            let moved = advance(w, i, &mut outbox);
            kernel_proof! {
                reveal(advance_post);
                if !changed && !moved {
                    assert(w.nodes@ == before_node@.nodes@);
                    assert forall|j: int| 0 <= j <= i implies !movable(*w, j) by {}
                }
                assert forall|j: int| 0 <= j < w.nodes@.len() implies
                    w.nodes@[j].generation as int + generation_capacity(w.nodes@[j])
                        <= baseline@.nodes@[j].generation as int + generation_capacity(baseline@.nodes@[j])
                    && w.nodes@[j].next_seq as int + effect_capacity(w.nodes@[j])
                        <= baseline@.nodes@[j].next_seq as int + effect_capacity(baseline@.nodes@[j])
                    && w.nodes@[j].credit.earned as int + merge_capacity(w.nodes@[j])
                        <= baseline@.nodes@[j].credit.earned as int + merge_capacity(baseline@.nodes@[j])
                    && w.nodes@[j].credit.start == baseline@.nodes@[j].credit.start
                    && w.nodes@[j].credit.added == baseline@.nodes@[j].credit.added
                    && w.nodes@[j].credit.spent == baseline@.nodes@[j].credit.spent by {
                    if j != i { assert(w.nodes@[j] == before_node@.nodes@[j]); }
                }
            }
            if moved {
                outbox.drain_into(out);
                changed = true;
                if let Some(t) = trace.as_mut() { t.push(snapshot_world(w)); }
            }
            i += 1;
        } }
        if !changed { return passes; }
        passes += 1;
    } }
}
}

/// One internal transition for node `i`, if the facts allow one.
/// Read scheduling predicates at the same branches as the local transition.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy)]
struct AdvanceContext {
    node: usize,
    admit: bool,
    pool: bool,
    slot: bool,
}

kernel_fn! {
/// #771: source-connected scheduler wrapper. Context queries are read at the
/// original branches; the verified local transition writes only this node.
fn advance(w: &mut World, i: usize, out: &mut Outbox) -> (result: bool) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        scheduling_safe(*old(w)),
        advance_pre(old(w).nodes@[i as int], old(w).next_ticket, old(out).len),
    ensures (!result ==> final(out).len == old(out).len),
        advance_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            old(w).next_ticket, final(w).next_ticket, result),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        movable(*old(w), i as int) ==> result,
        node_total(final(w).nodes@) <= node_total(old(w).nodes@),
        potential_prefix(final(w).nodes@, final(w).nodes@.len(), |n: Node| internal_height(n))
            <= potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
        result ==> potential_prefix(final(w).nodes@, final(w).nodes@.len(), |n: Node| internal_height(n))
            < potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
        final(w).next_ticket as int + potential_prefix(final(w).nodes@, final(w).nodes@.len(), |n: Node| ticket_capacity(n))
            <= old(w).next_ticket as int + potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| ticket_capacity(n)),
        open_prefix(final(w).nodes@, final(w).nodes@.len())
            <= open_prefix(old(w).nodes@, old(w).nodes@.len()),
        (forall|j: int| 0 <= j < old(w).nodes@.len() ==> settle_shape(old(w).nodes@[j]))
            ==> scheduling_safe(*final(w))
                && (forall|j: int| 0 <= j < final(w).nodes@.len() ==> settle_shape(final(w).nodes@[j])),
] {
    kernel_snapshot! { before_advance = *w }
    kernel_proof! { dependencies::prerequisite_definition(*w, NodeId(i as u64)); }
    let n = &w.nodes[i];
    let mut context = AdvanceContext {
        node: i,
        admit: false,
        pool: false,
        slot: false,
    };
    if !n.phase.is_closed() && !n.waiting_incident && n.effect.is_none() {
        match n.current() {
            Some(Item::Admit) if !n.paused => {
                context.admit = w.dependencies_delivered(NodeId(i as u64)) && admits(w, n.repo);
            }
            Some(Item::Start(_)) if !n.paused && n.ticket.is_some() => {
                context.pool = pool_free(w) && first_in_line(w, i);
            }
            Some(Item::Slot)
                if matches!(n.phase, Phase::Ready(_)) && !n.paused && n.ticket.is_some() =>
            {
                context.slot = slot_free(w, i) && first_in_line(w, i);
            }
            _ => {}
        }
    }
    kernel_proof! {
        assert(movable(*w, i as int) ==> advance_enabled(w.nodes@[i as int], context));
    }
    let changed = advance_node(&mut w.nodes[i], &mut w.next_ticket, context, out);
    kernel_proof! {
        reveal(advance_post);
        advance_sum_bounds(before_advance@.nodes@, w.nodes@, i as int,
            before_advance@.next_ticket, w.next_ticket, changed);
        if forall|j: int| 0 <= j < before_advance@.nodes@.len()
            ==> settle_shape(before_advance@.nodes@[j]) {
            assert forall|j: int| 0 <= j < w.nodes@.len()
                implies footprint::valid_footprint(*w, w.nodes@[j].footprint) by {
                footprint::valid_under_table_growth(before_advance@, *w, w.nodes@[j].footprint);
            }
            assert forall|j: int| 0 <= j < w.nodes@.len() implies settle_shape(w.nodes@[j]) by {}
            reveal(footprint::ordered_table);
        }
    }
    changed
}
}

kernel_fn! {
        /// A paused leaf starts nothing new. In a holding phase it gives back its slot
        /// and files and waits in Ready, resuming at the step it reached. Q30 / #771.
        // The pinned Verus frontend requires nested if-let instead of let chains.
        #[allow(clippy::collapsible_if)]
    fn pause_node(m: &mut Node) -> (result: bool) [
    requires holding(old(m).phase) ==> old(m).generation < u64::MAX,
    ensures final(m).generation as int <= old(m).generation as int + 1,
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        final(m).pool == old(m).pool,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).paused == old(m).paused, final(m).waiting_incident == old(m).waiting_incident,
        if holding(old(m).phase) {
            result && final(m).phase == Phase::Ready(match old(m).phase {
                Phase::Making => Resume::Making,
                Phase::Checking => Resume::Checking,
                Phase::Reviewing => Resume::Reviewing,
                Phase::Publishing | Phase::Queued => Resume::Publishing,
                _ => Resume::Rebasing,
            }) && final(m).pos == 0 && final(m).ticket.is_none()
                && final(m).effect == match old(m).effect {
                    Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                }
        } else {
            result == old(m).ticket.is_some()
                && *final(m) == (Node { ticket: None, ..*old(m) })
        },
    ] {
        if m.phase.holding() {
            if let Some(r) = m.phase.resume() {
                m.holds = false;
                set_phase(m, Phase::Ready(r));
                return true;
            }
        }
        if m.ticket.is_some() {
            m.ticket = None;
            return true;
        }
        false
    }
}

kernel_fn! {
    /// Q39 / #771: a place in line consumes exactly one global ticket.
    fn take_ticket_node(node: &mut Node, next_ticket: &mut u64) -> (result: ()) [
    requires *old(next_ticket) < u64::MAX,
    ensures *final(next_ticket) as int == *old(next_ticket) as int + 1,
        *final(node) == (Node { ticket: Some(*old(next_ticket)), ..*old(node) }),
    ] {
        let t = *next_ticket;
        *next_ticket += 1;
        node.ticket = Some(t);
    }
}

kernel_fn! {
/// Q39 / #771: exact pool occupancy, with the checked open-node scan bound.
fn pool_free(w: &World) -> (result: bool) [
    requires open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
        forall|i: int| 0 <= i < w.nodes@.len()
            ==> (w.nodes@[i].pool ==> !(w.nodes@[i].phase is Closed)),
    ensures result == (pool_users(w.nodes@, w.nodes@.len()) < w.pool_capacity),
] {
    let mut runs = 0u32;
    let mut i = 0usize;
    kernel_while! { (i < w.nodes.len()) [
        invariant
            i <= w.nodes.len(),
            runs as int == pool_users(w.nodes@, i as nat),
            runs as int <= open_prefix(w.nodes@, i as nat),
            open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
            forall|j: int| 0 <= j < w.nodes@.len()
                ==> (w.nodes@[j].pool ==> !(w.nodes@[j].phase is Closed)),
        decreases w.nodes.len() - i,
    ] {
        let n = &w.nodes[i];
        kernel_proof! {
            reveal(open_prefix);
            open_prefix_monotone(w.nodes@, i as nat + 1, w.nodes@.len());
        }
        if n.pool {
            runs += 1;
        }
        i += 1;
    } }
    runs < w.pool_capacity as u32
}
}

kernel_fn! {
/// Q30 / #771: planning admission uses the actual slot-waiter count.
fn admits(w: &World, repo: RepoId) -> (result: bool) [
    requires open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
    ensures result == if (repo.0 as int) < w.gates@.len() {
        slot_waiters(w.nodes@, w.nodes@.len(), repo) < w.gates@[repo.0 as int].w
    } else { false },
] {
    let Some(r) = repo.index() else {
        return false;
    };
    let Some(gate) = w.gates.get(r) else {
        return false;
    };
    let mut waiting = 0u32;
    let mut i = 0usize;
    kernel_while! { (i < w.nodes.len()) [
        invariant
            i <= w.nodes.len(),
            waiting as int == slot_waiters(w.nodes@, i as nat, repo),
            waiting as int <= open_prefix(w.nodes@, i as nat),
            open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
            r < w.gates.len(), r as int == repo.0 as int,
            *gate == w.gates@[r as int],
        decreases w.nodes.len() - i,
    ] {
        let n = &w.nodes[i];
        kernel_proof! {
            reveal(open_prefix);
            open_prefix_monotone(w.nodes@, i as nat + 1, w.nodes@.len());
        }
        if n.repo == repo && matches!(n.phase, Phase::Ready(_)) && n.ticket.is_some() {
            waiting += 1;
        }
        i += 1;
    } }
    waiting < gate.w as u32
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: storage and scratch-scan conditions read by the scheduling queries.
spec fn scheduling_safe(w: World) -> bool {
    footprint::ordered_table(w)
        && (forall|j: int| 0 <= j < w.nodes@.len()
            ==> footprint::valid_footprint(w, w.nodes@[j].footprint)
                && ((w.nodes@[j].holds || w.nodes@[j].pool)
                    ==> !(w.nodes@[j].phase is Closed)))
        && open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int
}


}

kernel_fn! {
/// Q30 / #771: a request has a free slot when no other request holds one.
/// The full scan still rejects overlaps and saturated widths at their old sites.
fn slot_free(w: &World, i: usize) -> (result: bool) [
    requires i < w.nodes@.len(), scheduling_safe(*w),
    ensures (w.nodes@[i as int].repo.0 < w.gates@.len()
        && w.gates@[w.nodes@[i as int].repo.0 as int].w > 0
        && (forall|j: int| 0 <= j < w.nodes@.len() && j != i ==> !w.nodes@[j].holds))
        ==> result,
] {
    let me = &w.nodes[i];
    let _gate_count = w.gates.len();
    let Some(repo) = me.repo.index() else { return false; };
    let Some(gate) = w.gates.get(repo) else { return false; };
    let mut holders = 0u32;
    let mut j = 0usize;
    kernel_while! { (j < w.nodes.len()) [
        invariant j <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *me == w.nodes@[i as int], repo < w.gates.len(),
            repo as int == me.repo.0 as int, *gate == w.gates@[repo as int],
            holders as int <= open_prefix(w.nodes@, j as nat),
            ((forall|k: int| 0 <= k < w.nodes@.len() && k != i ==> !w.nodes@[k].holds)
                ==> holders == 0),
        decreases w.nodes.len() - j,
    ] {
        let n = &w.nodes[j];
        kernel_proof! {
            reveal(open_prefix);
            open_prefix_monotone(w.nodes@, j as nat + 1, w.nodes@.len());
        }
        if j != i && n.holds && n.repo == me.repo {
            if !w.footprints_disjoint(n.footprint, me.footprint) {
                return false;
            }
            holders += 1;
        }
        j += 1;
    } }
    holders < gate.w as u32
}
}

/// A node's place in the agent pool's line (Q39), lowest first: its priority,
/// then the older ticket. None unless it waits for the pool.
pub fn pool_key(n: &Node) -> Option<(Priority, u64)> {
    let ticket = n.ticket.filter(|_| !matches!(n.phase, Phase::Ready(_)))?;
    Some((n.priority, ticket))
}

#[cfg(verus_keep_ghost)]
verus! {
spec fn priority_number(priority: Priority) -> nat {
    match priority {
        Priority::P0 => 0, Priority::P1 => 1, Priority::P2 => 2,
        Priority::P3 => 3, Priority::Unranked => 4,
    }
}

spec fn pool_precedes(a: Node, b: Node) -> bool {
    !(a.phase is Ready) && match (a.ticket, b.ticket) {
        (Some(theirs), Some(mine)) => priority_number(a.priority) < priority_number(b.priority)
            || (a.priority == b.priority && theirs < mine),
        _ => false,
    }
}

spec fn oldest_waiter(w: World, i: int) -> bool {
    w.nodes@[i].ticket.is_some() && forall|j: int| 0 <= j < w.nodes@.len() ==>
        if w.nodes@[i].phase is Ready {
            !(w.nodes@[j].phase is Ready) || w.nodes@[j].repo != w.nodes@[i].repo
                || match w.nodes@[j].ticket {
                    None => true, Some(theirs) => theirs >= w.nodes@[i].ticket.unwrap(),
                }
        } else { !pool_precedes(w.nodes@[j], w.nodes@[i]) }
}
}

kernel_fn! {
/// Q39 / #771: the declared priority order followed by the older pool ticket.
fn earlier_in_pool(a: &Node, b: &Node) -> (result: bool) [
    ensures result == pool_precedes(*a, *b),
] {
    if matches!(a.phase, Phase::Ready(_)) { return false; }
    match (a.ticket, b.ticket) {
        (Some(theirs), Some(mine)) => a.priority.rank() < b.priority.rank()
            || (a.priority == b.priority && theirs < mine),
        _ => false,
    }
}
}

kernel_fn! {
/// Q30 / #771: this is the original inner overlap scan, with explicit indices.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
fn older_overlap(w: &World, i: usize, ticket: u64) -> (result: bool) [
    requires i < w.nodes@.len(), scheduling_safe(*w),
] {
    let waiter = &w.nodes[i];
    let mut index = 0usize;
    kernel_while! { (index < w.nodes.len()) [
        invariant index <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *waiter == w.nodes@[i as int],
        decreases w.nodes.len() - index,
    ] {
        let older = &w.nodes[index];
        if matches!(older.phase, Phase::Ready(_)) && older.repo == waiter.repo {
            if let Some(theirs) = older.ticket {
                if theirs < ticket && !w.footprints_disjoint(older.footprint, waiter.footprint) {
                    return true;
                }
            }
        }
        index += 1;
    } }
    false
}
}

kernel_fn! {
/// Older slot waiters block runnable or overlapping younger leaves.
/// Q30/Q39 / #771: the oldest waiter is first in its actual executable line.
fn first_in_line(w: &World, i: usize) -> (result: bool) [
    requires i < w.nodes@.len(), scheduling_safe(*w),
    ensures oldest_waiter(*w, i as int) ==> result,
] {
    let me = &w.nodes[i];
    let Some(mine) = me.ticket else { return false };
    if !matches!(me.phase, Phase::Ready(_)) {
        let mut index = 0usize;
        kernel_while! { (index < w.nodes.len()) [
            invariant index <= w.nodes.len(), i < w.nodes.len(),
                *me == w.nodes@[i as int], !(me.phase is Ready), me.ticket == Some(mine),
            decreases w.nodes.len() - index,
        ] {
            if earlier_in_pool(&w.nodes[index], me) { return false; }
            index += 1;
        } }
        return true;
    }
    let mut first_runnable = mine;
    let mut j = 0usize;
    kernel_while! { (j < w.nodes.len()) [
        invariant j <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *me == w.nodes@[i as int], me.phase is Ready, me.ticket == Some(mine),
            oldest_waiter(*w, i as int) ==> first_runnable == mine,
        decreases w.nodes.len() - j,
    ] {
        let index = j;
        let waiter = &w.nodes[j];
        j += 1;
        let Some(theirs) = waiter.ticket else { continue; };
        if !matches!(waiter.phase, Phase::Ready(_)) || waiter.repo != me.repo || theirs >= mine {
            continue;
        }
        kernel_proof! { assert(!oldest_waiter(*w, i as int)); }
        if !w.footprints_disjoint(waiter.footprint, me.footprint) { return false; }
        if theirs >= first_runnable || !slot_free(w, index) { continue; }
        if !older_overlap(w, index, theirs) { first_runnable = theirs; }
    } }
    first_runnable == mine
}
}
