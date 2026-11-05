//! Publication decisions. The adapter retains every fence, read and write.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Approval,
    Publication,
    Message,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Steering {
    Correction,
    Restart,
    ConsoleNote,
    Other,
}
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

/// Escape: state-table.md:728-735, :743, :751, :755 at cdacce8f; Q27/Q49, #769/#766.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationRead {
    Reconciled,
    EvidencePermanent,
    OtherPermanent,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileAction {
    Preserve,
    FoundPermanent,
    Propagate,
    Pending,
}

/// Escape: state-table.md:727-730 at cdacce8f; Q27/Q54, #769/#766.
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

#[derive(Clone, Copy, Debug)]
pub struct OpenPrEvidence {
    pub open_or_merged: bool,
    pub valid_body: bool,
    pub valid_obligations: bool,
    pub no_gaps: bool,
    pub evidence_idempotent: bool,
}

/// Escape: state-table.md:728-730, :745 at cdacce8f; Q27/Q45/Q52, #769/#766.
/// Acceptance is exactly this conjunction, for every valuation, without a bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (e.open_or_merged && e.valid_body && e.valid_obligations && e.no_gaps && e.evidence_idempotent),
))]
pub fn accept_open_pr(e: OpenPrEvidence) -> bool {
    e.open_or_merged && e.valid_body && e.valid_obligations && e.no_gaps && e.evidence_idempotent
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalPreflight {
    Observed,
    Dispatch,
}

/// Escape: state-table.md:743 at cdacce8f; Q27, #769/#766 duplicate convergence.
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

#[derive(Clone, Copy, Debug)]
pub struct ApprovalFailure {
    pub sent: bool,
    pub permanent: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalFailureAction {
    Refused,
    Permanent,
    Uncertain,
}

/// Escape: state-table.md:727-730, :743, :753 at cdacce8f; Q27, #769/#766.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalObservation {
    Observed,
    Uncertain,
}

/// Escape: state-table.md:728-730, :743 at cdacce8f; Q27, #769/#766.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comment {
    Clarify,
    ReplacementDropped,
    Other,
}

/// Escape: state-table.md:751 at cdacce8f; Q27/Q49, #769/#766.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (notice || matches!(comment, Comment::Clarify | Comment::ReplacementDropped)),
))]
pub fn message_delivery(notice: bool, comment: Comment) -> bool {
    notice || matches!(comment, Comment::Clarify | Comment::ReplacementDropped)
}

#[derive(Clone, Copy, Debug)]
pub struct OperationsTarget {
    pub owner: bool,
    pub repository: bool,
    pub installation: bool,
    pub issue: bool,
}

/// Escape: state-table.md:915 at cdacce8f; Q17/Q27, #769/#766 incident routing.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleSource {
    Context,
    Issue,
}

/// Escape: state-table.md:678, :915 at cdacce8f; Q17/Q27/Q49, #769/#766.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRejection {
    Fallback,
    Permanent,
}

/// Escape: state-table.md:727, :751 at cdacce8f; Q27/Q54, #769/#766.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueRead {
    Closed,
    Open,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    Complete,
    Close,
    Pending,
}

/// Escape: state-table.md:696, :728-730 at cdacce8f; Q27/Q49, #769/#766.
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
