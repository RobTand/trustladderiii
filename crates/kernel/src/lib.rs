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
#[cfg(verus_keep_ghost)]
use ranks::*;

mod dependencies;
pub use dependencies::Dependency;
mod footprint;
mod headroom;
mod ids;
pub use headroom::*;
mod queries;
pub use footprint::Footprint;
pub use ids::{FileId, HeadId, NodeId, RepoId};
pub use queries::Wait;

#[cfg(kani)]
mod proofs;

/// Starting credit is the request's size in leaves plus this reserve.
pub const RESERVE: u32 = 3;
/// Rechecks after clean rebases that cost nothing, per leaf.
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
    pub fn outcome(self) -> Outcome {
        match self {
            EarlyEnd::Duplicate => Outcome::Duplicate,
            EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
            EarlyEnd::NotActionable => Outcome::NotActionable,
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

    fn can_spend(&self, cost: u32) -> bool {
        self.balance() >= cost as i64
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
                old(self).added as int + cost as int <= u32::MAX as int,
                old(self).spent as int + cost as int <= u32::MAX as int,
            ensures
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

    pub fn new(repo: RepoId) -> Node {
        Node {
            phase: Phase::Ingesting,
            pos: 0,
            repo,
            footprint: Footprint::default(),
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

    /// 0 for P0 through 4 for unranked.
    pub fn rank(self) -> u8 {
        self as u8
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

impl Outbox {
    pub const fn new() -> Self {
        Outbox {
            items: [None; OUTBOX],
            len: 0,
        }
    }

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

    fn drain_into(&self, out: &mut Vec<Out>) {
        out.extend(self.items.iter().flatten().copied());
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

/// Where a retried run starts again in its phase's agenda.
fn operation_start(phase: Phase) -> u8 {
    match phase {
        Phase::Making => 1,
        _ => 0,
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
fn notice(out: &mut Outbox, node: usize, notice: Notice) -> (result: ()) [
requires old(out).len < OUTBOX,
ensures final(out).len == old(out).len + 1,
] {
    out.push(Out::Notice {
        node: NodeId(node as u64),
        notice,
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

    fn index(&self, node: NodeId) -> Option<usize> {
        let i = node.index()?;
        if i < self.nodes.len() { Some(i) } else { None }
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
    pub fn is_settled(&self) -> bool {
        let mut w = self.clone();
        let mut out = Outbox::new();
        let mut i = 0;
        while i < w.nodes.len() {
            if advance(&mut w, i, &mut out) {
                return false;
            }
            i += 1;
        }
        true
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
    pub fn consistent(&self) -> bool {
        self.check().is_ok()
    }

    /// `consistent`, naming the first rule a world breaks.
    pub fn check(&self) -> Result<(), Broken> {
        self.check_dependencies()?;
        if self.gates.is_empty() || self.pool_capacity == 0 {
            return Err(Broken::Config);
        }
        for g in &self.gates {
            if g.w == 0 {
                return Err(Broken::Config);
            }
        }
        let mut i = 0;
        while i < self.nodes.len() {
            self.check_node(&self.nodes[i])?;
            i += 1;
        }
        // Locks are the footprints of holders in the same repo.
        let mut i = 0;
        while i < self.nodes.len() {
            let n = self.nodes[i];
            if n.holds {
                let mut j = 0;
                while j < i {
                    let other = self.nodes[j];
                    if other.holds
                        && other.repo == n.repo
                        && !self.footprints_disjoint(other.footprint, n.footprint)
                    {
                        return Err(Broken::Gate);
                    }
                    j += 1;
                }
            }
            i += 1;
        }
        let mut r = 0;
        while r < self.gates.len() {
            let mut count = 0usize;
            for n in &self.nodes {
                if n.holds && n.repo == RepoId(r as u64) {
                    count += 1;
                }
            }
            if count > self.gates[r].w as usize {
                return Err(Broken::Gate);
            }
            r += 1;
        }
        let mut runs = 0u32;
        for n in &self.nodes {
            if n.pool {
                runs += 1;
            }
        }
        if runs > self.pool_capacity as u32 {
            return Err(Broken::Pool);
        }
        for (a, x) in self.nodes.iter().enumerate() {
            for (b, y) in self.nodes.iter().enumerate() {
                if a != b && x.ticket.is_some() && x.ticket == y.ticket {
                    return Err(Broken::Ticket);
                }
            }
        }
        Ok(())
    }

    fn check_node(&self, n: &Node) -> Result<(), Broken> {
        let items = agenda(n.phase);
        if n.repo.index().is_none_or(|r| r >= self.gates.len()) {
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
                if n.effect.is_some_and(|e| !e.cancelling) {
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
            let live_effect = n.effect.is_some_and(|e| !e.cancelling);
            if live_effect || n.ticket.is_some() || n.phase.holding() || n.phase.is_closed() {
                return Err(Broken::Incident);
            }
            if !n.phase.is_stopping() && (n.run.is_some() || n.job.is_some() || n.pool) {
                return Err(Broken::Incident);
            }
        }

        if n.credit.withheld
            && n.question.is_some_and(|q| {
                !matches!(
                    q.cont,
                    Cont::Ingesting
                        | Cont::Auditing
                        | Cont::Unplanned
                        | Cont::Stop(_)
                        | Cont::Nowhere
                )
            })
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
            && n.effect
                .is_some_and(|e| !e.cancelling && e.kind == EffectKind::OpenPr);
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

    /// What a leaf needs before it may resume into a holding phase.
    fn check_ready(&self, n: &Node, r: Resume) -> Result<(), Broken> {
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
pub fn step(world: World, event: Event) -> (World, Vec<Out>) {
    let (world, out, _) = step_with_passes(world, event);
    (world, out)
}

/// Settle after the controller reconciles stored gate or agent capacity.
pub fn settle_world(mut world: World) -> (World, Vec<Out>) {
    let mut out = Vec::new();
    settle(&mut world, &mut out, None);
    (world, out)
}

/// `step`, also reporting how many passes settle needed. Tests use it to size
/// the loop bound that Kani's unwinding check then proves.
pub fn step_with_passes(mut world: World, event: Event) -> (World, Vec<Out>, u32) {
    let mut outbox = Outbox::new();
    apply(&mut world, event, &mut outbox);
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    let passes = settle(&mut world, &mut out, None);
    (world, out, passes)
}

/// `step`, also returning the world after the event and after every internal
/// transition. Tests check each one.
pub fn step_traced(mut world: World, event: Event) -> (World, Vec<Out>, Vec<World>) {
    let mut outbox = Outbox::new();
    let mut trace = Vec::new();
    apply(&mut world, event, &mut outbox);
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    trace.push(world.clone());
    settle(&mut world, &mut out, Some(&mut trace));
    (world, out, trace)
}

fn apply(w: &mut World, event: Event, out: &mut Outbox) {
    match event {
        Event::NewAdoption {
            repo,
            head,
            priority,
        } => {
            if repo.index().is_some_and(|r| r < w.gates.len()) {
                let mut node = Node::new(repo);
                node.proposed = Some(head);
                node.priority = priority;
                w.nodes.push(node);
            }
        }
        Event::NewRequest { repo, priority } => {
            if repo.index().is_some_and(|r| r < w.gates.len()) {
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
}

// ---------------------------------------------------------------- transitions

kernel_fn! {
    /// Every phase change starts a new generation, and the node stops waiting on
    /// whatever effect it had in flight. #771: one generation increment.
    fn set_phase(m: &mut Node, phase: Phase) -> (result: ()) [
        requires old(m).generation < u64::MAX,
        ensures
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

/// The run or job is over: release what it held.
fn abandon_operation(m: &mut Node) {
    m.run = None;
    m.job = None;
    m.pool = false;
    if let Some(e) = m.effect.as_mut()
        && e.kind.starts_operation()
    {
        e.cancelling = true;
    }
}

/// The run or job succeeded, which ends its failure lineage.
fn end_operation(m: &mut Node) {
    abandon_operation(m);
    m.run_failures = 0;
    m.run_incidents = 0;
}

fn enter_stopping(m: &mut Node, outcome: Outcome) {
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

kernel_fn! {
    /// Q18 / #771: merge credit is allocated once, increasing earned by at most one.
    fn merge_credit(m: &mut Node) -> (result: ()) [
        requires old(m).credit.merge_credited || old(m).credit.earned < u32::MAX,
        ensures
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

fn ask_rob(w: &mut World, i: usize, kind: QKind, cont: Cont, cost: u8, out: &mut Outbox) {
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

fn raise_incident(w: &mut World, i: usize, needs_rob: bool, out: &mut Outbox) {
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

fn emit(w: &mut World, i: usize, kind: EffectKind, out: &mut Outbox) {
    emit_node(&mut w.nodes[i], i, kind, out);
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

fn accepts(w: &World, n: &Node, reply: &Reply) -> bool {
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

fn on_reply(w: &mut World, i: usize, generation: u64, reply: Reply, out: &mut Outbox) {
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
        return;
    }
    if n.current() != Some(Item::Await) || (n.run != Some(generation) && n.job != Some(generation))
    {
        return; // stale
    }
    // A reply that breaks its schema or names the wrong commit counts as a failed run.
    if !accepts(w, n, &reply) {
        run_failed(w, i, out);
        return;
    }
    end_operation(&mut w.nodes[i]);
    match reply {
        Reply::IngestDone { size } => {
            let m = &mut w.nodes[i];
            if !m.credit.ready {
                m.credit.ready = true;
                m.credit.start = size as u32 + RESERVE;
            }
            m.early_end = None;
            set_phase(m, Phase::Auditing);
            w.withhold_dependency_credit(i);
        }
        Reply::IngestEarlyEnd(e) => {
            let m = &mut w.nodes[i];
            if !m.credit.ready {
                m.credit.ready = true;
                m.credit.start = 1 + RESERVE;
            }
            m.early_end = Some(e);
            set_phase(m, Phase::Auditing);
            w.withhold_dependency_credit(i);
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
        Reply::ProtectedPaths => enter_stopping(&mut w.nodes[i], Outcome::NotActionable),
        Reply::MakerDone { head } => {
            let m = &mut w.nodes[i];
            m.head = Some(head);
            m.checked = None;
            set_phase(m, Phase::Checking);
        }
        Reply::MakerNeedsFiles { files, fits } => {
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
        Reply::RunFailed => {}
    }
}

fn run_failed(w: &mut World, i: usize, out: &mut Outbox) {
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

/// Q54 (Rob, 2026-11-02): under `fail_fast`, a step that would retry the work
/// stops the request for TL3 review instead. It spends no credit and asks Rob
/// nothing; the summary offers `restart`, as for any stop (Q49).
fn stop_for_review(m: &mut Node, step: ReviewStop) {
    m.review_stop = Some(step);
    enter_stopping(m, Outcome::Stopped);
}

fn failed_round(w: &mut World, i: usize, step: ReviewStop, out: &mut Outbox) {
    let m = &mut w.nodes[i];
    if m.round == 0 {
        // The fix round resumes the maker's session. It's free.
        m.round = 1;
        set_phase(m, Phase::Making);
    } else {
        next_rung(w, i, step, out);
    }
}

/// The ladder after a failed fix round: a bigger budget, then a different
/// model, then one re-plan, then Rob. Each step costs 1.
fn next_rung(w: &mut World, i: usize, step: ReviewStop, out: &mut Outbox) {
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

fn out_of_room(w: &mut World, i: usize, step: ReviewStop, out: &mut Outbox) {
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

/// Under `fail_fast` the planner never re-runs on `cant_meet` (Q54). The
/// planner worker reports a broken rule as `cant_meet` too, so a plan that
/// doesn't fit (`cant_meet` false) can't arise live and keeps today's rule.
fn plan_retry(w: &mut World, i: usize, cant_meet: bool, out: &mut Outbox) {
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

/// A new head or a fresh reviewer uses the same free recheck allowance.
fn recheck(w: &mut World, i: usize, target: Phase, out: &mut Outbox) {
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

// ---------------------------------------------------------------- acknowledgements

fn on_ack(w: &mut World, i: usize, seq: u64, ack: Ack, out: &mut Outbox) {
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

fn confirmed(m: &mut Node, kind: EffectKind) {
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
    if !kind.starts_operation()
        && let Some(Item::Do(k) | Item::Teardown(k)) = m.current()
        && k == kind
    {
        if kind == EffectKind::PrepareWorktree
            && !m.adopted
            && let Some(head) = m.proposed
        {
            m.adopted = true;
            m.head = Some(head);
            m.checked = None;
            set_phase(m, Phase::Checking);
        } else {
            m.pos += 1;
        }
    }
}

fn delivery_failed(w: &mut World, i: usize, kind: EffectKind, out: &mut Outbox) {
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

fn effect_failed(w: &mut World, i: usize, kind: EffectKind, out: &mut Outbox) {
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

// ---------------------------------------------------------------- facts

fn on_fact(w: &mut World, i: usize, fact: Fact, out: &mut Outbox) {
    match fact {
        Fact::DependencyRemove { kind, number } => w.remove_dependency(i, kind, number),
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        Fact::Priority(priority) => {
            if !w.nodes[i].phase.is_closed() {
                w.nodes[i].priority = priority;
            }
        }
        Fact::PrMerged {
            head,
            checks_passed,
        } => pr_merged(w, i, head, checks_passed, out),
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
            if before_enqueue && let Some(effect) = m.effect.as_mut() {
                effect.cancelling = true;
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

fn pr_merged(w: &mut World, i: usize, head: HeadId, checks_passed: bool, out: &mut Outbox) {
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

// ---------------------------------------------------------------- commands

fn on_command(w: &mut World, i: usize, id: u64, command: Command, out: &mut Outbox) {
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

fn allowed(kind: QKind, answer: Answer) -> bool {
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

fn on_answer(w: &mut World, i: usize, id: u64, answer: Answer, out: &mut Outbox) {
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
                Answer::Continue => go_on(w, i, q, out),
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
                        out,
                    );
                }
                Answer::AddCredit(k) => {
                    w.nodes[i].credit.added += k as u32;
                    go_on(w, i, q, out);
                }
                Answer::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
                Answer::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
                _ => {}
            }
        }
        _ => {}
    }
}

/// Carry on to the question's continuation, paying what it costs.
fn go_on(w: &mut World, i: usize, q: Question, out: &mut Outbox) {
    let m = &mut w.nodes[i];
    m.credit.pay(q.cost as u32);
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

fn mark_handled(m: &mut Node) {
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

// ---------------------------------------------------------------- settle

/// Takes passes over the nodes until one changes nothing, and returns how many
/// passes changed something. Each internal transition moves a node along a
/// finite chain, so the loop ends. Kani's unwinding check proves it within the
/// bounds.
fn settle(w: &mut World, out: &mut Vec<Out>, mut trace: Option<&mut Vec<World>>) -> u32 {
    let mut passes: u32 = 0;
    loop {
        let mut changed = false;
        let mut i = 0;
        while i < w.nodes.len() {
            let mut outbox = Outbox::new();
            if advance(w, i, &mut outbox) {
                outbox.drain_into(out);
                changed = true;
                if let Some(t) = trace.as_mut() {
                    t.push(w.clone());
                }
            }
            i += 1;
        }
        if !changed {
            return passes;
        }
        passes += 1;
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

fn advance(w: &mut World, i: usize, out: &mut Outbox) -> bool {
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
    advance_node(&mut w.nodes[i], &mut w.next_ticket, context, out)
}

kernel_fn! {
    /// #771: actual local settle transition, for every scheduling decision.
fn advance_node(
    node: &mut Node,
    next_ticket: &mut u64,
    context: AdvanceContext,
    out: &mut Outbox,
)  -> (result: bool) [
    requires old(out).len + 2 <= OUTBOX,
        old(node).generation as int + generation_capacity(*old(node)) <= u64::MAX as int,
        old(node).next_seq as int + effect_capacity(*old(node)) <= u64::MAX as int,
        *old(next_ticket) as int + ticket_capacity(*old(node)) <= u64::MAX as int,
        old(node).credit.earned as int + merge_capacity(*old(node)) <= u32::MAX as int,
    ensures
        result ==> internal_height(*final(node)) < internal_height(*old(node)),
        super::ranks::distance(*final(node)) <= super::ranks::distance(*old(node)),
        result ==> 4 * super::ranks::distance(*final(node)) + pending_rank(*final(node))
            < 4 * super::ranks::distance(*old(node)) + pending_rank(*old(node)),
        final(node).generation as int + generation_capacity(*final(node))
            <= old(node).generation as int + generation_capacity(*old(node)),
        final(node).next_seq as int + effect_capacity(*final(node))
            <= old(node).next_seq as int + effect_capacity(*old(node)),
        *final(next_ticket) as int + ticket_capacity(*final(node))
            <= *old(next_ticket) as int + ticket_capacity(*old(node)),
        final(node).credit.earned as int + merge_capacity(*final(node))
            <= old(node).credit.earned as int + merge_capacity(*old(node)),
        final(node).credit.start == old(node).credit.start,
        final(node).credit.added == old(node).credit.added,
        final(node).credit.spent == old(node).credit.spent,
        !result ==> *final(node) == *old(node) && *final(next_ticket) == *old(next_ticket),
] {
    let n = *node;
    if n.phase.is_closed() {
        return false;
    }
    if n.waiting_incident {
        if n.ticket.is_some() {
            node.ticket = None;
            return true;
        }
        return false;
    }
    if n.effect.is_some() {
        return false; // one effect in flight at a time
    }
    let Some(item) = n.current() else {
        return match n.phase {
            Phase::Publishing => {
                set_phase(node, Phase::Queued);
                true
            }
            Phase::Stopping(outcome) if n.question.is_none() => {
                close_node(node, context.node, outcome, out);
                true
            }
            _ => false,
        };
    };
    match item {
        Item::Do(kind) => {
            if n.paused {
                return pause_node(node);
            }
            emit_node(node, context.node, kind, out);
            true
        }
        Item::Start(role) => {
            if n.paused {
                return pause_node(node);
            }
            if n.ticket.is_none() {
                take_ticket_node(node, next_ticket);
                return true;
            }
            if context.pool {
                let m = &mut *node;
                m.ticket = None;
                m.pool = true;
                m.run = Some(m.generation);
                m.pos += 1;
                emit_node(node, context.node, EffectKind::StartRun(role), out);
                return true;
            }
            false
        }
        Item::StartChecks => {
            if n.paused {
                return pause_node(node);
            }
            let m = &mut *node;
            m.job = Some(m.generation);
            m.pos += 1;
            emit_node(node, context.node, EffectKind::SubmitChecks, out);
            true
        }
        Item::StartRebase => {
            if n.paused {
                return pause_node(node);
            }
            let m = &mut *node;
            m.run = Some(m.generation);
            m.pos += 1;
            emit_node(node, context.node, EffectKind::StartRebase, out);
            true
        }
        Item::Await | Item::Merge | Item::Answer => false,
        Item::Admit => {
            if n.paused || !context.admit {
                return false;
            }
            let m = &mut *node;
            m.credit.withheld = false;
            m.planning_started = true;
            set_phase(m, Phase::Planning { must_split: false });
            true
        }
        Item::Slot => {
            let Phase::Ready(r) = n.phase else {
                return false;
            };
            if n.paused {
                if n.ticket.is_some() {
                    node.ticket = None;
                    return true;
                }
                return false;
            }
            if n.ticket.is_none() {
                take_ticket_node(node, next_ticket);
                return true;
            }
            if context.slot {
                let m = &mut *node;
                m.holds = true;
                set_phase(m, r.phase());
                return true;
            }
            false
        }
        Item::Release => {
            let m = &mut *node;
            m.holds = false;
            m.pos += 1;
            true
        }
        Item::Teardown(kind) => {
            if n.question.is_some() {
                return false; // waiting for Rob
            }
            let gone = match kind {
                EffectKind::StopRun => n.run.is_none(),
                EffectKind::WithdrawJob => n.job.is_none(),
                EffectKind::ClosePr => !n.pr_open,
                EffectKind::RemoveWorktree => !n.worktree,
                _ => false,
            };
            if gone {
                let m = &mut *node;
                if kind == EffectKind::StopRun {
                    m.pool = false;
                }
                m.pos += 1;
                return true;
            }
            emit_node(node, context.node, kind, out);
            true
        }
    }
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

fn pool_free(w: &World) -> bool {
    let mut runs = 0u32;
    for n in &w.nodes {
        if n.pool {
            runs += 1;
        }
    }
    runs < w.pool_capacity as u32
}

/// Planning is admitted while fewer than W leaves wait for a slot in the repo.
fn admits(w: &World, repo: RepoId) -> bool {
    let Some(gate) = repo.index().and_then(|r| w.gates.get(r)) else {
        return false;
    };
    let mut waiting = 0u32;
    for n in &w.nodes {
        if n.repo == repo && matches!(n.phase, Phase::Ready(_)) && n.ticket.is_some() {
            waiting += 1;
        }
    }
    waiting < gate.w as u32
}

fn slot_free(w: &World, i: usize) -> bool {
    let me = &w.nodes[i];
    let Some(gate) = me.repo.index().and_then(|r| w.gates.get(r)) else {
        return false;
    };
    let mut holders = 0u32;
    for (j, n) in w.nodes.iter().enumerate() {
        if j != i && n.holds && n.repo == me.repo {
            if !w.footprints_disjoint(n.footprint, me.footprint) {
                return false;
            }
            holders += 1;
        }
    }
    holders < gate.w as u32
}

/// A node's place in the agent pool's line (Q39), lowest first: its priority,
/// then the older ticket. None unless it waits for the pool.
pub fn pool_key(n: &Node) -> Option<(Priority, u64)> {
    let ticket = n.ticket.filter(|_| !matches!(n.phase, Phase::Ready(_)))?;
    Some((n.priority, ticket))
}

/// Older slot waiters block runnable or overlapping younger leaves.
/// The agent pool goes strictly by priority, then ticket (Q39).
fn first_in_line(w: &World, i: usize) -> bool {
    let me = &w.nodes[i];
    let Some(mine) = me.ticket else { return false };
    if !matches!(me.phase, Phase::Ready(_)) {
        return !w.nodes.iter().any(|n| {
            !matches!(n.phase, Phase::Ready(_))
                && n.ticket
                    .is_some_and(|theirs| (n.priority, theirs) < (me.priority, mine))
        });
    }
    let mut first_runnable = mine;
    for (j, waiter) in w.nodes.iter().enumerate() {
        let Some(theirs) = waiter.ticket else {
            continue;
        };
        if !matches!(waiter.phase, Phase::Ready(_)) || waiter.repo != me.repo || theirs >= mine {
            continue;
        }
        if !w.footprints_disjoint(waiter.footprint, me.footprint) {
            return false;
        }
        if theirs >= first_runnable || !slot_free(w, j) {
            continue;
        }
        // The oldest candidate with free files and no earlier overlap is
        // first in line. It blocks every younger ticket without recursion.
        let older_overlap = w.nodes.iter().any(|older| {
            matches!(older.phase, Phase::Ready(_))
                && older.repo == me.repo
                && older.ticket.is_some_and(|ticket| ticket < theirs)
                && !w.footprints_disjoint(older.footprint, waiter.footprint)
        });
        if !older_overlap {
            first_runnable = theirs;
        }
    }
    first_runnable == mine
}
