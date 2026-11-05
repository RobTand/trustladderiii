//! How a queue fact reaches the kernel. Setup evidence (Q52, #447 and
//! unreadable checks) only opens or clears incidents; it never becomes a
//! kernel fact, so it never counts a failure or charges credit.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// The request's node, as the translation reads it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Phase {
    Publishing,
    Queued,
    Closed,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct NodeView {
    pub phase: Phase,
    /// The kernel recorded a merge.
    pub merged: bool,
    /// The node's effect is an enqueue that is not being cancelled.
    pub enqueue_pending: bool,
}

impl NodeView {
    /// Publishing with an enqueue still pending.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == self.enqueuing_spec(),
))]
    fn enqueuing(self) -> bool {
        self.phase == Phase::Publishing && self.enqueue_pending
    }
}

/// A queue fact's evidence, without its payload.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Kind {
    ChecksUnavailable,
    CheckMissing,
    QueueBlocked,
    CheckReported,
    Merged { checks_passed: bool },
    CiFailed { head_check: bool, infra: bool },
    Conflict,
    DequeuedByCommand,
    DequeuedByPerson,
    ClosedByPerson,
    PushByOther,
}

impl Kind {
    /// Setup evidence is deduplicated against incidents, not observation receipts.
    pub fn setup(self) -> bool {
        matches!(
            self,
            Self::ChecksUnavailable | Self::CheckMissing | Self::CheckReported | Self::QueueBlocked
        )
    }

    /// Evidence about a queue attempt on one head, which matters only while
    /// that head is the node's head.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == self.names_attempt_head_spec(),
))]
    pub fn names_attempt_head(self) -> bool {
        matches!(
            self,
            Self::CiFailed { .. } | Self::Conflict | Self::DequeuedByCommand
        )
    }
}

/// Whether the node takes the fact at all. A closed node takes only a merge
/// it has not recorded. A head-check failure waits for a node that is queued
/// or still enqueuing, so a later active enqueue can consume it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (
        !(node.phase == Phase::Closed && (!matches!(kind, Kind::Merged { .. }) || node.merged))
        && (!matches!(kind, Kind::CiFailed { head_check: true, .. })
            || node.phase == Phase::Queued || node.enqueuing_spec())
    ),
))]
pub fn admit(kind: Kind, node: NodeView) -> bool {
    if node.phase == Phase::Closed && (!matches!(kind, Kind::Merged { .. }) || node.merged) {
        return false;
    }
    if let Kind::CiFailed {
        head_check: true, ..
    } = kind
    {
        return node.phase == Phase::Queued || node.enqueuing();
    }
    true
}

/// Why a queue CI failure is or is not infrastructure.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A test that failed in an earlier infrastructure failure on this head
    /// failed again, so it is the code.
    RepeatedTest,
    /// The failing tests lie outside the change.
    OutsideChange,
    /// Mergify's or the head checks' own classification stands.
    UpstreamClassification,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepeatedTest => "repeated_test",
            Self::OutsideChange => "outside_change",
            Self::UpstreamClassification => "upstream_classification",
        }
    }
}

/// Classify a queue CI failure. A repeated test is never infrastructure; a
/// failure outside the change is, unless repeated; otherwise `upstream` stands.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result.0 == (!repeated && (outside || upstream)), result.1 == (if repeated { Decision::RepeatedTest } else if outside { Decision::OutsideChange } else { Decision::UpstreamClassification }),
))]
pub fn classify_ci(upstream: bool, outside: bool, repeated: bool) -> (bool, Decision) {
    let infra = if outside || repeated {
        outside && !repeated
    } else {
        upstream
    };
    let decision = if repeated {
        Decision::RepeatedTest
    } else if outside {
        Decision::OutsideChange
    } else {
        Decision::UpstreamClassification
    };
    (infra, decision)
}

/// The kernel fact a queue fact becomes.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelFact {
    PrMerged { checks_passed: bool },
    CiFailed { infra: bool },
    Conflict,
    PrDequeuedByPerson,
    PrClosedByPerson,
    PushByOther,
}

/// What the controller does with an admitted queue fact.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Record the fact as observed; nothing else.
    Record,
    /// Open the check-setup incidents and record.
    SetupIncident,
    /// Q52: open the missing-check incident and record.
    MissingIncident,
    /// #447: open the queue-blocked incident and record.
    BlockedIncident,
    /// Q52: clear the open missing-check incident; nothing is recorded.
    ClearIncident,
    /// Drop it unrecorded, so a later read can raise it.
    Skip,
    /// Record and step the kernel.
    Kernel(KernelFact),
}

/// `head_current`: an attempt's head is the node's head. `approved_head`: a
/// Q52 or #447 fact names the node's approved head.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures
        result == route_spec(kind, node, head_current, approved_head),
        kind.names_attempt_head_spec() && !head_current ==> result == Route::Record,
        kind == Kind::ChecksUnavailable ==> result == Route::SetupIncident,
        kind == Kind::CheckMissing ==> result ==
            if node.enqueuing_spec() && approved_head { Route::MissingIncident } else { Route::Skip },
        kind == Kind::QueueBlocked ==> result ==
            if (node.phase == Phase::Queued || node.enqueuing_spec()) && approved_head {
                Route::BlockedIncident
            } else { Route::Skip },
        kind == Kind::CheckReported ==> result == Route::ClearIncident,
        matches!(kind, Kind::ChecksUnavailable | Kind::CheckMissing | Kind::QueueBlocked | Kind::CheckReported)
            ==> !matches!(result, Route::Kernel(_)),
))]
pub fn route(kind: Kind, node: NodeView, head_current: bool, approved_head: bool) -> Route {
    if kind.names_attempt_head() && !head_current {
        return Route::Record;
    }
    match kind {
        Kind::ChecksUnavailable => Route::SetupIncident,
        Kind::CheckMissing if node.enqueuing() && approved_head => Route::MissingIncident,
        Kind::QueueBlocked
            if (node.phase == Phase::Queued || node.enqueuing()) && approved_head =>
        {
            Route::BlockedIncident
        }
        Kind::CheckMissing | Kind::QueueBlocked => Route::Skip,
        Kind::CheckReported => Route::ClearIncident,
        Kind::Merged { checks_passed } => Route::Kernel(KernelFact::PrMerged { checks_passed }),
        Kind::CiFailed { infra, .. } => Route::Kernel(KernelFact::CiFailed { infra }),
        Kind::Conflict => Route::Kernel(KernelFact::Conflict),
        Kind::DequeuedByPerson | Kind::DequeuedByCommand => {
            Route::Kernel(KernelFact::PrDequeuedByPerson)
        }
        Kind::ClosedByPerson => Route::Kernel(KernelFact::PrClosedByPerson),
        Kind::PushByOther => Route::Kernel(KernelFact::PushByOther),
    }
}

#[cfg(verus_keep_ghost)]
verus! {
impl NodeView {
    /// #743/Q52: the production enqueuing predicate, open for composition.
    pub open spec fn enqueuing_spec(self) -> bool {
        self.phase == Phase::Publishing && self.enqueue_pending
    }
}
impl Kind {
    /// #743/K-translate-route: exactly these kinds name an attempt head.
    pub open spec fn names_attempt_head_spec(self) -> bool {
        matches!(self, Kind::CiFailed { .. } | Kind::Conflict | Kind::DequeuedByCommand)
    }
}
/// #743/K-translate-route and Q52: the full production routing relation.
pub open spec fn route_spec(kind: Kind, node: NodeView, head_current: bool, approved_head: bool) -> Route {
    if kind.names_attempt_head_spec() && !head_current { return Route::Record; }
    match kind {
        Kind::ChecksUnavailable => Route::SetupIncident,
        Kind::CheckMissing if node.enqueuing_spec() && approved_head => Route::MissingIncident,
        Kind::QueueBlocked if (node.phase == Phase::Queued || node.enqueuing_spec()) && approved_head => Route::BlockedIncident,
        Kind::CheckMissing | Kind::QueueBlocked => Route::Skip,
        Kind::CheckReported => Route::ClearIncident,
        Kind::Merged { checks_passed } => Route::Kernel(KernelFact::PrMerged { checks_passed }),
        Kind::CiFailed { infra, .. } => Route::Kernel(KernelFact::CiFailed { infra }),
        Kind::Conflict => Route::Kernel(KernelFact::Conflict),
        Kind::DequeuedByPerson | Kind::DequeuedByCommand => Route::Kernel(KernelFact::PrDequeuedByPerson),
        Kind::ClosedByPerson => Route::Kernel(KernelFact::PrClosedByPerson),
        Kind::PushByOther => Route::Kernel(KernelFact::PushByOther),
    }
}
}
