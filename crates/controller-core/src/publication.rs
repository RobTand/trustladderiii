//! Publication decisions. The adapter retains every fence, read and write.
//! All contracts quantify over the full typed input domain. These functions
//! have no loops or recursion, so verification needs no unwind or sequence bound.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Approval,
    Publication,
    Message,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Steering {
    Correction,
    Restart,
    ConsoleNote,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileRoute {
    Approval,
    Publication,
    Issue,
    Restart,
    ConsoleNote,
    Message,
    Records,
}

/// Escape: state-table.md:728-735, :744, :751, :755 at cdacce8f are effect/fence context.
/// Route precedence preserves github.rs:636-704 at cdacce8f (no rule).
/// Unbounded finite input domain. The shell fences before selecting this route.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match output {
        Output::Approval => ReconcileRoute::Approval,
        Output::Publication => ReconcileRoute::Publication,
        _ => match steering {
            Steering::Correction => ReconcileRoute::Issue,
            Steering::Restart => ReconcileRoute::Restart,
            Steering::ConsoleNote => ReconcileRoute::ConsoleNote,
            Steering::Other => match output { Output::Message => ReconcileRoute::Message, _ => ReconcileRoute::Records },
        },
    },
))]
pub fn reconcile_route(output: Output, steering: Steering) -> ReconcileRoute {
    match output {
        Output::Approval => ReconcileRoute::Approval,
        Output::Publication => ReconcileRoute::Publication,
        _ => match steering {
            Steering::Correction => ReconcileRoute::Issue,
            Steering::Restart => ReconcileRoute::Restart,
            Steering::ConsoleNote => ReconcileRoute::ConsoleNote,
            Steering::Other => match output {
                Output::Message => ReconcileRoute::Message,
                _ => ReconcileRoute::Records,
            },
        },
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationRead {
    Reconciled,
    EvidencePermanent,
    OtherPermanent,
    Failed,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileAction {
    Preserve,
    FoundPermanent,
    Propagate,
    Pending,
}

/// Escape: preserved existing behaviour (no rule): github.rs:640-651 at cdacce8f;
/// ee9e40cfe021 introduced pr_evidence -> Found(permanent).
/// Every permanent pr_evidence read is Found(permanent), never Pending.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match read {
        PublicationRead::Reconciled => ReconcileAction::Preserve,
        PublicationRead::EvidencePermanent => ReconcileAction::FoundPermanent,
        PublicationRead::OtherPermanent => ReconcileAction::Propagate,
        PublicationRead::Failed => ReconcileAction::Pending,
    },
))]
pub fn publication_reconciliation(read: PublicationRead) -> ReconcileAction {
    match read {
        PublicationRead::Reconciled => ReconcileAction::Preserve,
        PublicationRead::EvidencePermanent => ReconcileAction::FoundPermanent,
        PublicationRead::OtherPermanent => ReconcileAction::Propagate,
        PublicationRead::Failed => ReconcileAction::Pending,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
pub struct OpenPrEvidence {
    pub open_or_merged: bool,
    pub valid_body: bool,
    pub valid_obligations: bool,
    pub no_gaps: bool,
    pub evidence_idempotent: bool,
}

/// Escape: state-table.md:746 at cdacce8f is PR reconciliation context.
/// Exact acceptance preserves publishing.rs:393-408 at cdacce8f (no rule).
/// Acceptance is exactly this conjunction, for every valuation, without a bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (e.open_or_merged && e.valid_body && e.valid_obligations && e.no_gaps && e.evidence_idempotent),
))]
pub fn accept_open_pr(e: OpenPrEvidence) -> bool {
    e.open_or_merged && e.valid_body && e.valid_obligations && e.no_gaps && e.evidence_idempotent
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalPreflight {
    Observed,
    Dispatch,
}

/// Escape: state-table.md:744 at cdacce8f; explicit #766 decision in
/// state-machine.md:963-969 at cdacce8f; #769 duplicate convergence.
/// Called after all own-app copies have been PATCHed, not instead of convergence.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if converged { ApprovalPreflight::Observed } else { ApprovalPreflight::Dispatch },
))]
pub fn approval_preflight(converged: bool) -> ApprovalPreflight {
    if converged {
        ApprovalPreflight::Observed
    } else {
        ApprovalPreflight::Dispatch
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
pub struct ApprovalFailure {
    pub sent: bool,
    pub permanent: bool,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalFailureAction {
    Refused,
    Permanent,
    Uncertain,
}

/// Escape: state-table.md:728, :744, :753 at cdacce8f; explicit #766 decision
/// in state-machine.md:963-969; #769. Exact failure split preserves
/// publishing.rs:201-224 at cdacce8f (no additional rule).
/// A lost POST is sent and non-permanent: Uncertain, never Refused.
/// The adapter's earlier 404 Refused write is separate and remains in place.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if !failure.sent { ApprovalFailureAction::Refused }
        else if failure.permanent { ApprovalFailureAction::Permanent }
        else { ApprovalFailureAction::Uncertain },
))]
pub fn failed_approval(failure: ApprovalFailure) -> ApprovalFailureAction {
    if !failure.sent {
        ApprovalFailureAction::Refused
    } else if failure.permanent {
        ApprovalFailureAction::Permanent
    } else {
        ApprovalFailureAction::Uncertain
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalObservation {
    Observed,
    Uncertain,
}

/// Escape: state-table.md:744 at cdacce8f; explicit #766 decision in
/// state-machine.md:963-969. Exact evidence check preserves
/// publishing.rs:225-230 at cdacce8f (no additional rule).
/// A successful response with different approval evidence remains ambiguous.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if approved { ApprovalObservation::Observed } else { ApprovalObservation::Uncertain },
))]
pub fn observed_approval(approved: bool) -> ApprovalObservation {
    if approved {
        ApprovalObservation::Observed
    } else {
        ApprovalObservation::Uncertain
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comment {
    Clarify,
    ReplacementDropped,
    Other,
}

/// Escape: state-table.md:751 at cdacce8f is comment-delivery context.
/// Eligibility preserves github.rs:694-704, :1012-1024 at cdacce8f (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (notice || matches!(comment, Comment::Clarify | Comment::ReplacementDropped)),
))]
pub fn message_delivery(notice: bool, comment: Comment) -> bool {
    notice || matches!(comment, Comment::Clarify | Comment::ReplacementDropped)
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
pub struct OperationsTarget {
    pub owner: bool,
    pub repository: bool,
    pub installation: bool,
    pub issue: bool,
}

/// Escape: state-table.md:915 and Q17 (state-machine.md:502) at cdacce8f
/// require the operations issue. Exact identity check preserves
/// github.rs:1025-1036 at cdacce8f (no additional rule).
/// An incident is admitted iff every configured operations identity field matches.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (!incident || match target {
        Some(t) => t.owner && t.repository && t.installation && t.issue,
        None => false,
    }),
))]
pub fn incident_admitted(incident: bool, target: Option<OperationsTarget>) -> bool {
    !incident
        || match target {
            Some(t) => t.owner && t.repository && t.installation && t.issue,
            None => false,
        }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleSource {
    Context,
    Issue,
}

/// Escape: state-table.md:678, :915; Q17/Q49 (state-machine.md:502/:479)
/// at cdacce8f are incident/restart context. Exact title-source predicate
/// preserves github.rs:1046-1052 at cdacce8f (no additional rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if incident || restart_dropped { TitleSource::Context } else { TitleSource::Issue },
))]
pub fn title_source(incident: bool, restart_dropped: bool) -> TitleSource {
    if incident || restart_dropped {
        TitleSource::Context
    } else {
        TitleSource::Issue
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRejection {
    Fallback,
    Permanent,
}

/// Escape: preserved existing behaviour (no rule): github.rs:1097-1114 at
/// cdacce8f; 60e70bc7d280 introduced the Clarify-only rephrase fallback.
/// Only Clarify receives the fixed rephrase request after message rejection.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match comment { Comment::Clarify => MessageRejection::Fallback, _ => MessageRejection::Permanent },
))]
pub fn rejected_message(comment: Comment) -> MessageRejection {
    match comment {
        Comment::Clarify => MessageRejection::Fallback,
        _ => MessageRejection::Permanent,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueRead {
    Closed,
    Open,
    Failed,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    Complete,
    Close,
    Pending,
}

/// Escape: state-table.md:696, :728-730; Q49 (state-machine.md:479) at
/// cdacce8f are closure/reconciliation context. Exact read/close split
/// preserves github.rs:714-731 at cdacce8f (no additional rule).
/// A found summary does not confirm a failed read or a still-open close target.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match issue { IssueRead::Closed => CloseAction::Complete, IssueRead::Open => CloseAction::Close, IssueRead::Failed => CloseAction::Pending },
))]
pub fn close_after_reconcile(issue: IssueRead) -> CloseAction {
    match issue {
        IssueRead::Closed => CloseAction::Complete,
        IssueRead::Open => CloseAction::Close,
        IssueRead::Failed => CloseAction::Pending,
    }
}
