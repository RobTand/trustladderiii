#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Intended,
    Sent,
    Uncertain,
    Confirmed,
    Failed,
    Cancelling,
    Cancelled,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    Confirmed,
    Retryable,
    Permanent,
    BodyPolicyRefused,
}

/// An ordering sampled by the shell, generic over its epoch representation.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochOrder {
    Older,
    Current,
    Newer,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionPlan {
    pub bridge: bool,
    pub from: State,
    pub to: State,
}

/// State-table Effects :755, F1. The comparison and file I/O stay in the shell.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == !matches!(order, EpochOrder::Older),
))]
pub fn accepts_fence(order: EpochOrder) -> bool {
    !matches!(order, EpochOrder::Older)
}

/// State-table Effects :755. Existence is sampled only when this returns false.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == matches!(order, EpochOrder::Newer),
))]
pub fn advances_fence(order: EpochOrder) -> bool {
    matches!(order, EpochOrder::Newer)
}

/// State-table Effects :725-734. The bridge is staged in the completion transaction.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.bridge == (state == State::Uncertain && ack != Ack::Confirmed),
    out.from == if out.bridge { State::Sent } else { state },
    out.to == if state == State::Cancelling || ack == Ack::Confirmed {
        State::Confirmed
    } else if ack == Ack::Retryable { State::Intended } else { State::Failed },
))]
pub fn completion(state: State, ack: Ack) -> CompletionPlan {
    let bridge = state == State::Uncertain && ack != Ack::Confirmed;
    CompletionPlan {
        bridge,
        from: if bridge { State::Sent } else { state },
        to: if state == State::Cancelling {
            State::Confirmed
        } else {
            match ack {
                Ack::Confirmed => State::Confirmed,
                Ack::Retryable => State::Intended,
                Ack::Permanent | Ack::BodyPolicyRefused => State::Failed,
            }
        },
    }
}

/// State-table Effects :724-734. No final state has an outgoing edge.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match from {
        State::Intended => matches!(to, State::Sent | State::Cancelled),
        State::Sent => matches!(to, State::Confirmed | State::Intended | State::Failed
            | State::Uncertain | State::Cancelling),
        State::Uncertain => matches!(to, State::Confirmed | State::Intended | State::Cancelling),
        State::Cancelling => matches!(to, State::Confirmed | State::Cancelled),
        _ => false,
    },
))]
pub fn allows(from: State, to: State) -> bool {
    use State::*;
    matches!(
        (from, to),
        (Intended, Sent | Cancelled)
            | (Sent, Confirmed | Intended | Failed | Uncertain | Cancelling)
            | (Uncertain, Confirmed | Intended | Cancelling)
            | (Cancelling, Confirmed | Cancelled)
    )
}

/// State-table Effects :731-734 and store ActiveDeliveries, F2.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == matches!(state, State::Sent | State::Uncertain | State::Cancelling),
))]
pub fn blocks_epoch_advance(state: State) -> bool {
    matches!(state, State::Sent | State::Uncertain | State::Cancelling)
}

/// Q27 / state-machine approval retries: clear authorizes only an active attempt.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == matches!(state, State::Sent | State::Uncertain),
))]
pub fn can_authorize_approval_retry(state: State) -> bool {
    matches!(state, State::Sent | State::Uncertain)
}

/// Q27: retire ambiguity only after repair accounts for all durable dispatches.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (copies >= dispatches),
))]
pub fn accounts_for_dispatches(copies: u64, dispatches: u64) -> bool {
    copies >= dispatches
}

#[cfg(not(verus_keep_ghost))]
macro_rules! delivery_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! delivery_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

/// State-table Effects, ack by key: equality covers the entire original byte key.
/// Unbounded key length, with a decreasing finite slice length.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (left@ == right@),
))]
pub fn key_matches(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    delivery_while! { (index < left.len()) [
        invariant index <= left.len(), left.len() == right.len(),
            forall|i: int| 0 <= i < index ==> left@[i] == right@[i],
        decreases left.len() - index,
    ] {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }}
    #[cfg(verus_keep_ghost)]
    vstd::prelude::verus_exec_expr! { proof { assert(left@ =~= right@); } }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    /// State-table Effects :755, F1: equality is an accepted entry epoch.
    #[test]
    fn equal_epoch_is_admitted() {
        assert!(accepts_fence(EpochOrder::Current));
    }
}

/// Snapshot facts sampled by the interpreter. No payload or durable row is owned here.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
pub struct DeliveryView<'a> {
    pub key: &'a [u8],
    pub epoch: u64,
    pub revision: u64,
    pub state: State,
    pub fence: Fence,
    pub waits: Waits,
    pub approval: ApprovalGate,
    pub kind: DeliveryKind,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fence { Live, Retiring }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
pub struct Waits { pub outage: bool, pub quota: bool, pub held: bool }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalGate { Ordinary, Fresh, Authorized, Prior { predecessor: bool } }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryKind { KernelEffect, Auxiliary }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence { Found(Ack), Absent, Pending, Error }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Acknowledged(Ack), Ambiguous, Unanswered }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    History, SentExecute, SentFound(Ack), SentAmbiguous,
    ObserveExecute, ObserveReconcile, Execute, Reconcile,
}

/// Volatile, single-use permission. Only this module constructs it. Restart loses it.
/// The key is borrowed from the opaque delivery, never reconstructed from a payload.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Debug)]
pub struct Continuation<'a> {
    key: &'a [u8], epoch: u64, revision: u64, stage: Stage,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Debug)]
pub enum Input<'a> {
    Start,
    History { pending: Continuation<'a>, evidence: Evidence },
    /// Supply only after committing Sent and deleting the grant atomically, then reloading.
    SentCommitted(Continuation<'a>),
    Observed { pending: Continuation<'a>, revision_changed: bool },
    Executed { pending: Continuation<'a>, outcome: Outcome },
    Reconciled { pending: Continuation<'a>, evidence: Evidence },
    /// `approval_successor` describes an actual approval effect emitted by the kernel.
    Consume { key: &'a [u8], ack: Ack, kernel_ack: Ack, approval_successor: bool },
    ClearApproval { key: &'a [u8], matching_open_incident: bool },
    CheckedApprovalNews { key: &'a [u8] },
    RepairDone { copies: Option<u64>, dispatches: u64 },
}

/// Completion edges are bridge U→I→S, completion.from→to, then optional I→Cancelled.
/// All edges and bookkeeping belong to one transaction at the original key.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Debug)]
pub struct CommitPlan {
    pub completion: CompletionPlan,
    pub kernel_ack: Ack,
    pub retire_retry: bool,
    pub remove_watch: bool,
    pub authorize_successor: bool,
    pub retire_approval: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Debug)]
pub enum Action<'a> {
    Wait,
    Return,
    ReadHistory(Continuation<'a>),
    CommitSent { spend_grant: bool, then: Continuation<'a> },
    Observe(Continuation<'a>),
    Execute(Continuation<'a>),
    Reconcile(Continuation<'a>),
    Transition(State),
    EnqueueCompletion(Ack),
    /// The adapter owns cancellation admission, including the Mergify gate.
    CancelAbsent,
    RecordAmbiguity { stop: bool },
    /// Refusal, holds, quota classification and result metadata stay adapter I/O.
    RecordUnanswered,
    Commit(CommitPlan),
    /// Atomically restore Intended, authorization and lineage at this key.
    AuthorizeRetry,
    ScheduleRepair,
    FinishRepair { clear_unresolved: bool },
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition { AwaitInput, Continue, ContinueProgress, ReturnProgress, AmbiguityProgress }

impl Action<'_> {
    /// AmbiguityProgress uses the actual ambiguity transaction's changed result.
    pub fn disposition(&self) -> Disposition {
        match self {
            Action::ReadHistory(_) | Action::CommitSent { .. } | Action::Observe(_)
            | Action::Execute(_) | Action::Reconcile(_) => Disposition::AwaitInput,
            Action::Wait => Disposition::Continue,
            Action::Transition(State::Intended) | Action::RecordUnanswered => Disposition::ContinueProgress,
            Action::RecordAmbiguity { stop: false } => Disposition::AmbiguityProgress,
            _ => Disposition::ReturnProgress,
        }
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.key@ == view.key@, out.epoch == view.epoch,
    out.revision == view.revision, out.stage == stage,
))]
fn continuation(view: DeliveryView<'_>, stage: Stage) -> Continuation<'_> {
    Continuation { key: view.key, epoch: view.epoch, revision: view.revision, stage }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (view.key@ == pending.key@ && view.epoch == pending.epoch
        && view.revision == pending.revision),
))]
fn bound(view: DeliveryView<'_>, pending: &Continuation<'_>) -> bool {
    key_matches(view.key, pending.key) && view.epoch == pending.epoch && view.revision == pending.revision
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    match out {
        Action::CommitSent { spend_grant, then } => spend_grant == (view.approval != ApprovalGate::Ordinary)
            && then.key@ == view.key@ && then.epoch == view.epoch
            && then.revision == view.revision && then.stage == stage,
        _ => false,
    },
))]
fn commit_sent(view: DeliveryView<'_>, stage: Stage) -> Action<'_> {
    Action::CommitSent { spend_grant: view.approval != ApprovalGate::Ordinary, then: continuation(view, stage) }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    match view.approval {
        ApprovalGate::Prior { .. } => match out { Action::ReadHistory(p) =>
            p.key@ == view.key@ && p.epoch == view.epoch && p.revision == view.revision && p.stage == Stage::History, _ => false },
        _ => match out { Action::CommitSent { spend_grant, then } =>
            spend_grant == (view.approval != ApprovalGate::Ordinary) && then.key@ == view.key@
            && then.epoch == view.epoch && then.revision == view.revision && then.stage == Stage::SentExecute, _ => false },
    },
))]
fn intended(view: DeliveryView<'_>) -> Action<'_> {
    match view.approval {
        ApprovalGate::Prior { .. } => Action::ReadHistory(continuation(view, Stage::History)),
        _ => commit_sent(view, Stage::SentExecute),
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    match out {
        Action::Observe(p) => view.fence == Fence::Live && p.stage == Stage::ObserveReconcile
            && p.key@ == view.key@ && p.epoch == view.epoch && p.revision == view.revision,
        Action::Reconcile(p) => view.fence == Fence::Retiring && !view.waits.quota
            && p.stage == Stage::Reconcile && p.key@ == view.key@ && p.epoch == view.epoch && p.revision == view.revision,
        Action::Wait => view.fence == Fence::Retiring && view.waits.quota,
        _ => false,
    },
))]
fn begin_reconciliation(view: DeliveryView<'_>) -> Action<'_> {
    if view.fence == Fence::Live { Action::Observe(continuation(view, Stage::ObserveReconcile)) }
    else if view.waits.quota { Action::Wait }
    else { Action::Reconcile(continuation(view, Stage::Reconcile)) }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    start_contract(view, out),
))]
fn start(view: DeliveryView<'_>) -> Action<'_> {
    if view.fence == Fence::Retiring {
        match view.state {
            State::Intended => return Action::CancelAbsent,
            State::Sent | State::Uncertain => return Action::Transition(State::Cancelling),
            _ => {},
        }
    }
    if view.waits.outage && view.fence == Fence::Live { return Action::Wait; }
    match view.state {
        State::Intended => intended(view),
        State::Sent => Action::Transition(State::Uncertain),
        State::Uncertain | State::Cancelling => begin_reconciliation(view),
        State::Confirmed | State::Failed | State::Cancelled => Action::Wait,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    history_contract(view, pending, evidence, out),
))]
fn history<'a>(view: DeliveryView<'a>, pending: Continuation<'a>, evidence: Evidence) -> Action<'a> {
    if !bound(view, &pending) || pending.stage != Stage::History || view.state != State::Intended
        || view.fence != Fence::Live || view.waits.outage { return Action::Wait; }
    match view.approval {
        ApprovalGate::Prior { predecessor } => {
            let stage = if predecessor { Stage::SentAmbiguous } else {
                match evidence {
                    Evidence::Absent => Stage::SentExecute,
                    Evidence::Found(ack) => Stage::SentFound(ack),
                    Evidence::Pending | Evidence::Error => Stage::SentAmbiguous,
                }
            };
            commit_sent(view, stage)
        }
        _ => Action::Wait,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    sent_contract(view, pending, out),
))]
fn sent_committed<'a>(view: DeliveryView<'a>, pending: Continuation<'a>) -> Action<'a> {
    // The Sent transaction changes revision; the following reload establishes
    // the revision against which observation and issue are bound.
    if !key_matches(view.key, pending.key) || view.epoch != pending.epoch
        || view.revision < pending.revision || view.state != State::Sent { return Action::Wait; }
    match pending.stage {
        Stage::SentExecute => Action::Observe(continuation(view, Stage::ObserveExecute)),
        Stage::SentFound(ack) => Action::EnqueueCompletion(ack),
        Stage::SentAmbiguous => Action::RecordAmbiguity { stop: true },
        _ => Action::Wait,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    observed_contract(view, pending, revision_changed, out),
))]
fn observed<'a>(view: DeliveryView<'a>, pending: Continuation<'a>, revision_changed: bool) -> Action<'a> {
    if !bound(view, &pending) { return Action::Wait; }
    if !matches!(pending.stage, Stage::ObserveExecute | Stage::ObserveReconcile) { return Action::Wait; }
    if revision_changed { return Action::Return; }
    match pending.stage {
        Stage::ObserveExecute if view.state == State::Sent => Action::Execute(continuation(view, Stage::Execute)),
        Stage::ObserveReconcile if matches!(view.state, State::Uncertain | State::Cancelling) => {
            if view.waits.quota { Action::Wait } else { Action::Reconcile(continuation(view, Stage::Reconcile)) }
        }
        _ => Action::Wait,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    executed_contract(view, pending, outcome, out),
))]
fn executed<'a>(view: DeliveryView<'a>, pending: Continuation<'a>, outcome: Outcome) -> Action<'a> {
    if !bound(view, &pending) || pending.stage != Stage::Execute || view.state != State::Sent { return Action::Wait; }
    match outcome {
        Outcome::Acknowledged(ack) => Action::EnqueueCompletion(ack),
        Outcome::Ambiguous => Action::RecordAmbiguity { stop: true },
        Outcome::Unanswered => Action::RecordUnanswered,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    reconciled_contract(view, pending, evidence, out),
))]
fn reconciled<'a>(view: DeliveryView<'a>, pending: Continuation<'a>, evidence: Evidence) -> Action<'a> {
    if !bound(view, &pending) || pending.stage != Stage::Reconcile
        || !matches!(view.state, State::Uncertain | State::Cancelling) { return Action::Wait; }
    match evidence {
        Evidence::Found(ack) => Action::EnqueueCompletion(ack),
        Evidence::Absent => {
            if view.state == State::Cancelling { Action::CancelAbsent }
            else if !view.waits.held { Action::Transition(State::Intended) }
            else { Action::Wait }
        }
        Evidence::Pending => {
            if view.approval != ApprovalGate::Ordinary && view.fence == Fence::Live && !view.waits.held {
                Action::RecordAmbiguity { stop: false }
            } else { Action::Wait }
        }
        Evidence::Error => Action::Wait,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    consume_contract(view, key, ack, kernel_ack, approval_successor, out),
))]
fn consume<'a>(view: DeliveryView<'a>, key: &[u8], ack: Ack, kernel_ack: Ack, approval_successor: bool) -> Action<'a> {
    if !key_matches(view.key, key) || !matches!(view.state, State::Sent | State::Uncertain | State::Cancelling) { return Action::Wait; }
    let plan = completion(view.state, ack);
    let retire_retry = view.kind == DeliveryKind::KernelEffect && ack == Ack::Retryable && plan.to == State::Intended;
    Action::Commit(CommitPlan {
        completion: plan, kernel_ack, retire_retry, remove_watch: retire_retry,
        authorize_successor: retire_retry && view.approval != ApprovalGate::Ordinary && approval_successor,
        retire_approval: view.approval != ApprovalGate::Ordinary && (retire_retry || matches!(plan.to, State::Confirmed | State::Failed | State::Cancelled)),
    })
}

/// Pure protocol step. I/O results must come from the operation named by the
/// consumed continuation. No token is serializable or Clone. Enqueue does not
/// change effect state; Consume alone plans the completion transaction.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    action_contract(view, input, out),
))]
pub fn action<'a>(view: DeliveryView<'a>, input: Input<'a>) -> Action<'a> {
    match input {
        Input::Start => start(view),
        Input::History { pending, evidence } => history(view, pending, evidence),
        Input::SentCommitted(pending) => sent_committed(view, pending),
        Input::Observed { pending, revision_changed } => observed(view, pending, revision_changed),
        Input::Executed { pending, outcome } => executed(view, pending, outcome),
        Input::Reconciled { pending, evidence } => reconciled(view, pending, evidence),
        Input::Consume { key, ack, kernel_ack, approval_successor } => consume(view, key, ack, kernel_ack, approval_successor),
        Input::ClearApproval { key, matching_open_incident } => {
            if key_matches(view.key, key) && matching_open_incident && view.approval != ApprovalGate::Ordinary
                && can_authorize_approval_retry(view.state) { Action::AuthorizeRetry } else { Action::Wait }
        }
        Input::CheckedApprovalNews { key } => {
            if key_matches(view.key, key) && view.approval != ApprovalGate::Ordinary { Action::ScheduleRepair } else { Action::Wait }
        }
        Input::RepairDone { copies, dispatches } => {
            if view.approval == ApprovalGate::Ordinary { return Action::Wait; }
            let clear_unresolved = match copies { Some(copies) => accounts_for_dispatches(copies, dispatches), None => false };
            Action::FinishRepair { clear_unresolved }
        }
    }
}

#[cfg(verus_keep_ghost)]
verus! {
pub closed spec fn permission_key(p: Continuation) -> Seq<u8> { p.key@ }
pub closed spec fn permission_epoch(p: Continuation) -> u64 { p.epoch }
pub closed spec fn permission_revision(p: Continuation) -> u64 { p.revision }
pub closed spec fn permission_stage(p: Continuation) -> Stage { p.stage }

pub open spec fn permission_at(p: Continuation, v: DeliveryView) -> bool {
    permission_key(p) == v.key@ && permission_epoch(p) == v.epoch && permission_revision(p) == v.revision
}

pub open spec fn sent_action(v: DeliveryView, stage: Stage, out: Action) -> bool {
    match out {
        Action::CommitSent { spend_grant, then } => spend_grant == (v.approval != ApprovalGate::Ordinary)
            && permission_at(then, v) && permission_stage(then) == stage,
        _ => false,
    }
}

pub open spec fn start_contract(v: DeliveryView, out: Action) -> bool {
    if v.fence == Fence::Retiring && v.state == State::Intended { matches!(out, Action::CancelAbsent) }
    else if v.fence == Fence::Retiring && matches!(v.state, State::Sent | State::Uncertain) {
        matches!(out, Action::Transition(State::Cancelling))
    } else if v.waits.outage && v.fence == Fence::Live { matches!(out, Action::Wait) }
    else { match v.state {
        State::Intended => match v.approval {
            ApprovalGate::Prior { .. } => match out { Action::ReadHistory(p) => permission_at(p, v) && permission_stage(p) == Stage::History, _ => false },
            _ => sent_action(v, Stage::SentExecute, out),
        },
        State::Sent => matches!(out, Action::Transition(State::Uncertain)),
        State::Uncertain | State::Cancelling => match out {
            Action::Observe(p) => v.fence == Fence::Live && permission_at(p, v) && permission_stage(p) == Stage::ObserveReconcile,
            Action::Reconcile(p) => v.fence == Fence::Retiring && !v.waits.quota && permission_at(p, v) && permission_stage(p) == Stage::Reconcile,
            Action::Wait => v.fence == Fence::Retiring && v.waits.quota,
            _ => false,
        },
        _ => matches!(out, Action::Wait),
    }}
}

pub open spec fn history_contract(v: DeliveryView, p: Continuation, e: Evidence, out: Action) -> bool {
    if permission_at(p, v) && permission_stage(p) == Stage::History && v.state == State::Intended
        && v.fence == Fence::Live && !v.waits.outage {
        match v.approval {
            ApprovalGate::Prior { predecessor } => sent_action(v,
                if predecessor { Stage::SentAmbiguous } else { match e {
                    Evidence::Absent => Stage::SentExecute,
                    Evidence::Found(ack) => Stage::SentFound(ack),
                    _ => Stage::SentAmbiguous,
                }}, out),
            _ => matches!(out, Action::Wait),
        }
    } else { matches!(out, Action::Wait) }
}

pub open spec fn sent_contract(v: DeliveryView, p: Continuation, out: Action) -> bool {
    if v.key@ == permission_key(p) && v.epoch == permission_epoch(p) && v.revision >= permission_revision(p) && v.state == State::Sent {
        match permission_stage(p) {
            Stage::SentExecute => match out { Action::Observe(next) => permission_at(next, v) && permission_stage(next) == Stage::ObserveExecute, _ => false },
            Stage::SentFound(ack) => matches!(out, Action::EnqueueCompletion(a) if a == ack),
            Stage::SentAmbiguous => matches!(out, Action::RecordAmbiguity { stop: true }),
            _ => matches!(out, Action::Wait),
        }
    } else { matches!(out, Action::Wait) }
}

pub open spec fn observed_contract(v: DeliveryView, p: Continuation, changed: bool, out: Action) -> bool {
    if !permission_at(p, v) || !matches!(permission_stage(p), Stage::ObserveExecute | Stage::ObserveReconcile) { matches!(out, Action::Wait) }
    else if changed { matches!(out, Action::Return) }
    else { match permission_stage(p) {
        Stage::ObserveExecute => if v.state == State::Sent {
            match out { Action::Execute(next) => permission_at(next, v) && permission_stage(next) == Stage::Execute, _ => false }
        } else { matches!(out, Action::Wait) },
        _ => if matches!(v.state, State::Uncertain | State::Cancelling) && !v.waits.quota {
            match out { Action::Reconcile(next) => permission_at(next, v) && permission_stage(next) == Stage::Reconcile, _ => false }
        } else { matches!(out, Action::Wait) },
    }}
}

pub open spec fn executed_contract(v: DeliveryView, p: Continuation, outcome: Outcome, out: Action) -> bool {
    if permission_at(p, v) && permission_stage(p) == Stage::Execute && v.state == State::Sent {
        match outcome {
            Outcome::Acknowledged(ack) => matches!(out, Action::EnqueueCompletion(a) if a == ack),
            Outcome::Ambiguous => matches!(out, Action::RecordAmbiguity { stop: true }),
            Outcome::Unanswered => matches!(out, Action::RecordUnanswered),
        }
    } else { matches!(out, Action::Wait) }
}

pub open spec fn reconciled_contract(v: DeliveryView, p: Continuation, e: Evidence, out: Action) -> bool {
    if permission_at(p, v) && permission_stage(p) == Stage::Reconcile && matches!(v.state, State::Uncertain | State::Cancelling) {
        match e {
            Evidence::Found(ack) => matches!(out, Action::EnqueueCompletion(a) if a == ack),
            Evidence::Absent => if v.state == State::Cancelling { matches!(out, Action::CancelAbsent) }
                else if !v.waits.held { matches!(out, Action::Transition(State::Intended)) }
                else { matches!(out, Action::Wait) },
            Evidence::Pending => if v.approval != ApprovalGate::Ordinary && v.fence == Fence::Live && !v.waits.held {
                matches!(out, Action::RecordAmbiguity { stop: false })
            } else { matches!(out, Action::Wait) },
            Evidence::Error => matches!(out, Action::Wait),
        }
    } else { matches!(out, Action::Wait) }
}

pub open spec fn consume_contract(v: DeliveryView, key: &[u8], ack: Ack, kernel_ack: Ack, successor: bool, out: Action) -> bool {
    if v.key@ == key@ && matches!(v.state, State::Sent | State::Uncertain | State::Cancelling) {
        match out { Action::Commit(p) =>
            p.completion.bridge == (v.state == State::Uncertain && ack != Ack::Confirmed)
            && p.completion.from == (if p.completion.bridge { State::Sent } else { v.state })
            && p.completion.to == (if v.state == State::Cancelling || ack == Ack::Confirmed { State::Confirmed }
                else if ack == Ack::Retryable { State::Intended } else { State::Failed })
            && p.kernel_ack == kernel_ack
            && p.retire_retry == (v.kind == DeliveryKind::KernelEffect && ack == Ack::Retryable && v.state != State::Cancelling)
            && p.remove_watch == p.retire_retry
            && p.authorize_successor == (p.retire_retry && v.approval != ApprovalGate::Ordinary && successor)
            && p.retire_approval == (v.approval != ApprovalGate::Ordinary && (p.retire_retry || matches!(p.completion.to, State::Confirmed | State::Failed | State::Cancelled))),
            _ => false,
        }
    } else { matches!(out, Action::Wait) }
}

pub open spec fn action_contract(v: DeliveryView, input: Input, out: Action) -> bool {
    match input {
        Input::Start => start_contract(v, out),
        Input::History { pending, evidence } => history_contract(v, pending, evidence, out),
        Input::SentCommitted(pending) => sent_contract(v, pending, out),
        Input::Observed { pending, revision_changed } => observed_contract(v, pending, revision_changed, out),
        Input::Executed { pending, outcome } => executed_contract(v, pending, outcome, out),
        Input::Reconciled { pending, evidence } => reconciled_contract(v, pending, evidence, out),
        Input::Consume { key, ack, kernel_ack, approval_successor } => consume_contract(v, key, ack, kernel_ack, approval_successor, out),
        Input::ClearApproval { key, matching_open_incident } =>
            if v.key@ == key@ && matching_open_incident && v.approval != ApprovalGate::Ordinary && matches!(v.state, State::Sent | State::Uncertain) {
                matches!(out, Action::AuthorizeRetry)
            } else { matches!(out, Action::Wait) },
        Input::CheckedApprovalNews { key } => if v.key@ == key@ && v.approval != ApprovalGate::Ordinary {
            matches!(out, Action::ScheduleRepair)
        } else { matches!(out, Action::Wait) },
        Input::RepairDone { copies, dispatches } => if v.approval == ApprovalGate::Ordinary { matches!(out, Action::Wait) }
            else { matches!(out, Action::FinishRepair { clear_unresolved } if clear_unresolved == match copies { Some(c) => c >= dispatches, None => false }) },
    }
}
}
