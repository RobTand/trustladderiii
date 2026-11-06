//! Read-only mirror of controller lib.rs:1231-1438. Scheduling, observations,
//! store loads, durable transitions, execute and reconcile remain separate events.
use crate::{Ack, State, completion};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Found,
    Absent,
    Pending,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Wait,
    Send,
    Uncertain,
    Cancel,
    Cancelling,
    Complete,
    Retry,
    Ambiguous,
}

/// State-table :724-734. Special queue shutdown and approval history checks run
/// before this ordinary per-effect branch; their conformance is scoped separately.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if cancelling && state == State::Intended { Action::Cancel }
    else if cancelling && matches!(state, State::Sent | State::Uncertain) { Action::Cancelling }
    else if outage && !cancelling { Action::Wait }
    else if state == State::Intended { Action::Send }
    else if state == State::Sent { Action::Uncertain }
    else { Action::Wait },
))]
pub fn dispatch(state: State, cancelling: bool, outage: bool) -> Action {
    if cancelling {
        match state {
            State::Intended => return Action::Cancel,
            State::Sent | State::Uncertain => return Action::Cancelling,
            _ => {}
        }
    }
    if outage && !cancelling {
        return Action::Wait;
    }
    match state {
        State::Intended => Action::Send,
        State::Sent => Action::Uncertain,
        _ => Action::Wait,
    }
}

/// State-table reconciliation rows, including holds. Found is handled by the
/// real completion plan; Pending is never an acknowledgement or evidence of absence.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match read {
        Read::Found => Action::Complete,
        Read::Absent => if state == State::Cancelling { Action::Cancel }
            else if !held { Action::Retry } else { Action::Wait },
        Read::Pending => if approval && !cancelling && !held { Action::Ambiguous }
            else { Action::Wait },
    },
))]
pub fn reconciliation(
    state: State,
    read: Read,
    held: bool,
    approval: bool,
    cancelling: bool,
) -> Action {
    match read {
        Read::Found => Action::Complete,
        Read::Absent if state == State::Cancelling => Action::Cancel,
        Read::Absent if !held => Action::Retry,
        Read::Pending if approval && !cancelling && !held => Action::Ambiguous,
        _ => Action::Wait,
    }
}

/// State-table Effects: only the caller's separately committed action changes state.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match action {
        Action::Send => State::Sent,
        Action::Uncertain => State::Uncertain,
        Action::Cancel => State::Cancelled,
        Action::Cancelling => State::Cancelling,
        Action::Retry => State::Intended,
        Action::Complete => if state == State::Cancelling || ack == Ack::Confirmed {
            State::Confirmed
        } else if ack == Ack::Retryable { State::Intended } else { State::Failed },
        _ => state,
    },
))]
pub fn committed(state: State, action: Action, ack: Ack) -> State {
    match action {
        Action::Send => State::Sent,
        Action::Uncertain => State::Uncertain,
        Action::Cancel => State::Cancelled,
        Action::Cancelling => State::Cancelling,
        Action::Retry => State::Intended,
        Action::Complete => completion(state, ack).to,
        _ => state,
    }
}

/// Q27 / lib.rs:1270-1293: an authorization bypasses prior-history recovery.
/// Without it, a prior ambiguous dispatch cannot POST again; authoritative
/// Absent is valid only for preflight/refused attempts (publishing.rs:518-532).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if authorized || !prior_dispatch {Action::Send}
        else if !predecessor && read == Read::Absent {Action::Send}
        else if !predecessor && read == Read::Found {Action::Complete}
        else {Action::Ambiguous},
))]
pub fn approval_dispatch(
    prior_dispatch: bool,
    predecessor: bool,
    authorized: bool,
    read: Read,
) -> Action {
    if authorized || !prior_dispatch {
        return Action::Send;
    }
    match read {
        Read::Absent if !predecessor => Action::Send,
        Read::Found if !predecessor => Action::Complete,
        _ => Action::Ambiguous,
    }
}

/// An issuer retains this process-local attempt across ownership loss. The store
/// row can subsequently be drained by a new owner; `durably_sent` is historical
/// evidence, not a claim that the row remains Sent after recovery.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Issuer {
    pub epoch: u64,
    pub durably_sent: bool,
    pub owns: bool,
    pub in_call: bool,
    pub admitted: bool,
    pub issued: bool,
}

/// State-table :724-734 / lib.rs:1293-1300: durable Sent precedes adapter entry.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires state == State::Sent,
    ensures out.epoch == epoch, out.durably_sent, out.owns, out.in_call,
        !out.admitted, !out.issued,
))]
pub fn begin_issuer(state: State, epoch: u64) -> Issuer {
    Issuer {
        epoch,
        durably_sent: state == State::Sent,
        owns: true,
        in_call: true,
        admitted: false,
        issued: false,
    }
}

/// State-table :755 F1: entry read is a distinct sample, before any fence write
/// or HTTP read/write. Another owner may advance the fence after this sample.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.admitted == (before.epoch >= sampled_fence),
    out.epoch == before.epoch, out.durably_sent == before.durably_sent,
    out.owns == before.owns, out.in_call == before.in_call, out.issued == before.issued,
))]
pub fn entry(before: Issuer, sampled_fence: u64) -> Issuer {
    let order = if before.epoch < sampled_fence {
        crate::EpochOrder::Older
    } else if before.epoch == sampled_fence {
        crate::EpochOrder::Current
    } else {
        crate::EpochOrder::Newer
    };
    Issuer {
        admitted: crate::accepts_fence(order),
        ..before
    }
}

/// Store lib.rs:73-80 F3: ownership loss does not erase an issuer or its HTTP call.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    !out.owns, out.epoch == before.epoch, out.durably_sent == before.durably_sent,
    out.admitted == before.admitted, out.in_call == before.in_call, out.issued == before.issued,
))]
pub fn lose_ownership(before: Issuer) -> Issuer {
    Issuer {
        owns: false,
        ..before
    }
}

/// F3: no issue-time fence is assumed. This mirrors the real checked-entry then
/// GET/Dispatching/POST ordering. A zombie still has the original Sent evidence.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires before.durably_sent, before.admitted, before.in_call,
    ensures out.issued, out.durably_sent, out.epoch == before.epoch,
        out.admitted == before.admitted, out.owns == before.owns,
        out.in_call == before.in_call,
))]
pub fn issue_http(before: Issuer) -> Issuer {
    Issuer {
        issued: true,
        ..before
    }
}

/// lib.rs:1282/1287: approval history has a separate durable Sent commit,
/// before completion enqueue or ambiguity. Even the Found branch cannot skip it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryPath {
    pub sent: State,
    pub outcome: Action,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.sent == State::Sent,
    out.outcome == if authorized || !prior {Action::Send}
        else if !predecessor && read == Read::Absent {Action::Send}
        else if !predecessor && read == Read::Found {Action::Complete}
        else {Action::Ambiguous},
))]
pub fn approval_history(
    prior: bool,
    predecessor: bool,
    authorized: bool,
    read: Read,
) -> HistoryPath {
    HistoryPath {
        sent: State::Sent,
        outcome: approval_dispatch(prior, predecessor, authorized, read),
    }
}

/// lib.rs:1842-1875: completion's failure bridge and old-key cancellation are
/// staged in one input transaction. The kernel may create a replacement sequence;
/// no replacement is fabricated if the kernel instead enters failure teardown.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if kernel_effect && ack == Ack::Retryable && state != State::Cancelling {
        State::Cancelled
    } else if state == State::Cancelling || ack == Ack::Confirmed {State::Confirmed}
    else if ack == Ack::Retryable {State::Intended} else {State::Failed},
))]
pub fn consumed_completion(state: State, ack: Ack, kernel_effect: bool) -> State {
    let planned = completion(state, ack).to;
    if kernel_effect && ack == Ack::Retryable && planned == State::Intended {
        State::Cancelled
    } else {
        planned
    }
}

/// input_errors.rs:478-503,235-258: durable input enqueue and consumption are
/// separate. A process crash retains queued=true until a committed consumption.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionInput {
    pub state: State,
    pub queued: bool,
    pub ack: Ack,
    pub kernel_effect: bool,
    pub replacement: bool,
    pub replacement_grant: bool,
    pub watch: bool,
    pub retirement: bool,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.queued, out.state == before.state, out.ack == ack,
    out.kernel_effect == before.kernel_effect, out.replacement == before.replacement,
    out.replacement_grant == before.replacement_grant, out.watch == before.watch,
    out.retirement == before.retirement,
))]
pub fn enqueue_completion(before: CompletionInput, ack: Ack) -> CompletionInput {
    CompletionInput {
        queued: true,
        ack,
        ..before
    }
}
/// `kernel_replacement` is sampled from the real kernel outputs in apply, not
/// from network absence. Only a retryable completion authorizes those successors.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires before.queued,
    ensures !out.queued,
        out.state == if before.kernel_effect && before.ack == Ack::Retryable && before.state != State::Cancelling {
            State::Cancelled
        } else if before.state == State::Cancelling || before.ack == Ack::Confirmed {State::Confirmed}
        else if before.ack == Ack::Retryable {State::Intended} else {State::Failed},
        out.replacement == (before.kernel_effect && before.ack == Ack::Retryable
            && before.state != State::Cancelling && kernel_replacement),
        out.replacement_grant == (approval && out.replacement),
        out.retirement == (before.retirement ||
            (before.kernel_effect && before.ack == Ack::Retryable && before.state != State::Cancelling)
            || (approval && matches!(out.state, State::Confirmed | State::Failed | State::Cancelled))),
        out.watch == (before.watch && !(before.kernel_effect && before.ack == Ack::Retryable && before.state != State::Cancelling)),
))]
pub fn consume_completion(
    before: CompletionInput,
    approval: bool,
    kernel_replacement: bool,
) -> CompletionInput {
    let retry =
        before.kernel_effect && before.ack == Ack::Retryable && before.state != State::Cancelling;
    let state = consumed_completion(before.state, before.ack, before.kernel_effect);
    CompletionInput {
        state,
        queued: false,
        replacement: retry && kernel_replacement,
        replacement_grant: approval && retry && kernel_replacement,
        watch: before.watch && !retry,
        retirement: before.retirement
            || retry
            || (approval && matches!(state, State::Confirmed | State::Failed | State::Cancelled)),
        ..before
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    None,
    HumanClear,
    RetryableReplacement,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Approval {
    pub state: State,
    pub grant: Grant,
    pub lineage_safe: bool,
    pub unresolved: bool,
    pub incident_open: bool,
    pub repair: bool,
}
/// approval_delivery.rs:94-123: a matching open ambiguity incident and an active
/// pending attempt are the provenance for a human grant, Intended and safe lineage.
/// One transition models one atomic input transaction, not three independent rows.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if before.incident_open && matches!(before.state, State::Sent | State::Uncertain) {
        Approval { state: State::Intended, grant: Grant::HumanClear, lineage_safe: true,
            incident_open: false, ..before }
    } else { before },
))]
pub fn clear_approval(before: Approval) -> Approval {
    if before.incident_open && crate::can_authorize_approval_retry(before.state) {
        Approval {
            state: State::Intended,
            grant: Grant::HumanClear,
            lineage_safe: true,
            incident_open: false,
            ..before
        }
    } else {
        before
    }
}
/// lib.rs:1626-1630: Sent and deletion of AUTHORIZATIONS are ONE commit. A crash
/// immediately before retains both Intended and the grant; immediately after retains
/// Sent and no grant, even if observations prevent the HTTP call from being issued.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.state == State::Sent, out.grant == Grant::None,
    out.lineage_safe == before.lineage_safe, out.unresolved == before.unresolved,
    out.incident_open == before.incident_open, out.repair == before.repair,
))]
pub fn send_approval(before: Approval) -> Approval {
    Approval {
        state: State::Sent,
        grant: Grant::None,
        ..before
    }
}
/// lib.rs:1853-1866: only an actual retryable kernel completion can supply a
/// successor authorization. The new key is distinct; the old key is Cancelled.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires consumed.replacement_grant,
    ensures out.state == State::Intended, out.grant == Grant::RetryableReplacement,
        out.lineage_safe, !out.incident_open, !out.unresolved, !out.repair,
))]
pub fn replacement_approval(consumed: CompletionInput) -> Approval {
    let _ = consumed;
    Approval {
        state: State::Intended,
        grant: Grant::RetryableReplacement,
        lineage_safe: true,
        incident_open: false,
        unresolved: false,
        repair: false,
    }
}
/// approval_delivery.rs:57-88: ambiguity creates UNRESOLVED plus the incident;
/// creating an incident never creates an authorization.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.unresolved, out.incident_open, out.grant == before.grant,
    out.state == before.state, out.lineage_safe == before.lineage_safe, out.repair == before.repair,
))]
pub fn ambiguous_approval(before: Approval) -> Approval {
    Approval {
        unresolved: true,
        incident_open: true,
        ..before
    }
}
/// Approval sidecar at publishing.rs:518-532. No dispatch evidence or an empty
/// GET alone proves a prior POST ended. Preflight/Refused are the safe absence cases.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sidecar {
    Preflight,
    Refused,
    Dispatching,
    Uncertain,
    Unknown,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if found {Read::Found} else if matches!(sidecar, Sidecar::Preflight | Sidecar::Refused) {Read::Absent} else {Read::Pending},
))]
pub fn approval_evidence(sidecar: Sidecar, found: bool) -> Read {
    if found {
        Read::Found
    } else if matches!(sidecar, Sidecar::Preflight | Sidecar::Refused) {
        Read::Absent
    } else {
        Read::Pending
    }
}
/// drive_once:1270: grant is read from the same loaded snapshot as the effect.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if before.grant != Grant::None || !prior {Action::Send}
        else if !predecessor && read == Read::Absent {Action::Send}
        else if !predecessor && read == Read::Found {Action::Complete}
        else {Action::Ambiguous},
))]
pub fn approval_decision(before: Approval, prior: bool, predecessor: bool, read: Read) -> Action {
    approval_dispatch(prior, predecessor, before.grant != Grant::None, read)
}

/// store:2182 excludes finals. Recovery is scheduled through existing durable
/// watches/teardown, restart's closed-node look, and approval/message news rows.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    None,
    Pending,
    Watch,
    Teardown,
    ClosedNodeLook,
    ApprovalNews,
    MessageNews,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if matches!(state, State::Intended | State::Sent | State::Uncertain | State::Cancelling) {Recovery::Pending}
    else if closed_restart {Recovery::ClosedNodeLook}
    else if teardown {Recovery::Teardown}
    else if watch {Recovery::Watch} else {Recovery::None},
))]
pub fn recovery(state: State, closed_restart: bool, teardown: bool, watch: bool) -> Recovery {
    if matches!(
        state,
        State::Intended | State::Sent | State::Uncertain | State::Cancelling
    ) {
        Recovery::Pending
    } else if closed_restart {
        Recovery::ClosedNodeLook
    } else if teardown {
        Recovery::Teardown
    } else if watch {
        Recovery::Watch
    } else {
        Recovery::None
    }
}
/// lib.rs:1297-1298/1381: a revision-changing observation returns before HTTP,
/// retaining the previously committed Sent and any queued input.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !revision_changed))]
pub fn continue_after_observations(revision_changed: bool) -> bool {
    !revision_changed
}

/// approval_delivery.rs:126-209: identity-checked news queues REPAIRS even when
/// the effect is final and absent from pending_effects.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.repair, out.state == before.state, out.grant == before.grant,
    out.lineage_safe == before.lineage_safe, out.unresolved == before.unresolved,
    out.incident_open == before.incident_open,
))]
pub fn approval_news(before: Approval) -> Approval {
    Approval {
        repair: true,
        ..before
    }
}

/// approval_delivery.rs:255-281: successful repair consumes REPAIRS; UNRESOLVED
/// persists until identity-checked copies account for every durable Sent intent.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    !out.repair, out.unresolved == (before.unresolved && copies < dispatches),
    out.state == before.state, out.grant == before.grant,
    out.lineage_safe == before.lineage_safe, out.incident_open == before.incident_open,
))]
pub fn repaired_approval(before: Approval, copies: u64, dispatches: u64) -> Approval {
    Approval {
        repair: false,
        unresolved: before.unresolved && !crate::accounts_for_dispatches(copies, dispatches),
        ..before
    }
}

/// One admitted controller invocation, including the grant spent at its Sent
/// commit. This volatile permission is distinct from the durable grant row.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovalAttempt {
    pub durable: Approval,
    pub may_issue: bool,
    pub issued: bool,
    pub spent: Grant,
}
/// lib.rs:1270-1298. Gate on the snapshot grant/history, commit Sent (and its
/// grant deletion), then permit HTTP only on the Send path. Found/Pending history
/// still commits Sent but cannot issue. Observations may discard this permission.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.may_issue == (before.state == State::Intended &&
        (before.grant != Grant::None || !prior || (!predecessor && read == Read::Absent))),
    out.durable.state == if before.state == State::Intended {State::Sent} else {before.state},
    out.durable.grant == if before.state == State::Intended {Grant::None} else {before.grant},
    out.durable.lineage_safe == before.lineage_safe,
    out.durable.unresolved == before.unresolved,
    out.durable.incident_open == before.incident_open,
    out.durable.repair == before.repair,
    out.spent == if before.state == State::Intended {before.grant} else {Grant::None}, !out.issued,
))]
pub fn begin_approval_attempt(
    before: Approval,
    prior: bool,
    predecessor: bool,
    read: Read,
) -> ApprovalAttempt {
    if before.state != State::Intended {
        return ApprovalAttempt {
            durable: before,
            may_issue: false,
            issued: false,
            spent: Grant::None,
        };
    }
    let action = approval_decision(before, prior, predecessor, read);
    ApprovalAttempt {
        durable: send_approval(before),
        may_issue: action == Action::Send,
        issued: false,
        spent: before.grant,
    }
}
/// F3/Q27: the admitted invocation can issue after ownership loss. Its durable
/// Sent is historical evidence. A single invocation consumes its volatile issue
/// permission; it never recreates the durable authorization row.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires before.may_issue, before.durable.state == State::Sent,
    ensures !out.may_issue, out.issued, out.durable == before.durable, out.spent == before.spent,
))]
pub fn issue_approval_attempt(before: ApprovalAttempt) -> ApprovalAttempt {
    ApprovalAttempt {
        may_issue: false,
        issued: true,
        ..before
    }
}
