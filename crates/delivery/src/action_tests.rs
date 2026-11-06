use super::*;

fn view(state: State) -> DeliveryView<'static> {
    DeliveryView {
        key: b"original", epoch: 3, revision: 10, state,
        fence: Fence::Live, waits: Waits { outage: false, quota: false, held: false },
        approval: ApprovalGate::Ordinary, kind: DeliveryKind::KernelEffect,
    }
}

fn sent(v: DeliveryView<'static>) -> (DeliveryView<'static>, Continuation<'static>) {
    let Action::CommitSent { spend_grant, then } = action(v, Input::Start) else { panic!("Sent before I/O") };
    assert_eq!(spend_grant, v.approval != ApprovalGate::Ordinary);
    (DeliveryView { state: State::Sent, revision: v.revision + 1, ..v }, then)
}

fn reconcile(v: DeliveryView<'static>, evidence: Evidence) -> Action<'static> {
    let next = action(v, Input::Start);
    let pending = match next {
        Action::Observe(pending) => {
            let Action::Reconcile(pending) = action(v, Input::Observed { pending, revision_changed: false }) else { panic!("reconcile after observe") };
            pending
        }
        Action::Reconcile(pending) => pending,
        _ => panic!("reconciliation required"),
    };
    action(v, Input::Reconciled { pending, evidence })
}

#[test]
fn retiring_precedes_outage_and_adapter_owns_cancellation_gate() {
    for state in [State::Intended, State::Sent, State::Uncertain] {
        let mut v = view(state); v.fence = Fence::Retiring; v.waits.outage = true;
        match action(v, Input::Start) {
            Action::CancelAbsent => assert_eq!(state, State::Intended),
            Action::Transition(State::Cancelling) => assert_ne!(state, State::Intended),
            _ => panic!("retirement first"),
        }
    }
}

#[test]
fn live_outage_waits_and_finals_are_excluded() {
    for state in [State::Intended, State::Sent, State::Uncertain, State::Cancelling] {
        let mut v = view(state); v.waits.outage = true;
        assert!(matches!(action(v, Input::Start), Action::Wait));
    }
    for state in [State::Confirmed, State::Failed, State::Cancelled] {
        assert!(matches!(action(view(state), Input::Start), Action::Wait));
    }
}

#[test]
fn dispatch_requires_sent_commit_and_observation_without_revision_change() {
    for approval in [ApprovalGate::Ordinary, ApprovalGate::Fresh, ApprovalGate::Authorized] {
        let v = DeliveryView { approval, ..view(State::Intended) };
        let (v, pending) = sent(v);
        let Action::Observe(pending) = action(v, Input::SentCommitted(pending)) else { panic!("observe first") };
        let Action::Execute(pending) = action(v, Input::Observed { pending, revision_changed: false }) else { panic!("execute admitted") };
        assert!(matches!(action(v, Input::Executed { pending, outcome: Outcome::Acknowledged(Ack::Confirmed) }), Action::EnqueueCompletion(Ack::Confirmed)));
        let (v, pending) = sent(DeliveryView { state: State::Intended, ..v });
        let Action::Observe(pending) = action(v, Input::SentCommitted(pending)) else { panic!() };
        assert!(matches!(action(v, Input::Observed { pending, revision_changed: true }), Action::Return));
    }
}

#[test]
fn approval_history_commits_sent_for_found_pending_errors_and_predecessors() {
    for predecessor in [false, true] {
        for evidence in [Evidence::Absent, Evidence::Found(Ack::Confirmed), Evidence::Pending, Evidence::Error] {
            let v = DeliveryView { approval: ApprovalGate::Prior { predecessor }, ..view(State::Intended) };
            let Action::ReadHistory(pending) = action(v, Input::Start) else { panic!() };
            let Action::CommitSent { spend_grant, then } = action(v, Input::History { pending, evidence }) else { panic!("history commits Sent") };
            assert!(spend_grant);
            let next = action(DeliveryView { state: State::Sent, revision: 11, ..v }, Input::SentCommitted(then));
            if predecessor || matches!(evidence, Evidence::Pending | Evidence::Error) {
                assert!(matches!(next, Action::RecordAmbiguity { stop: true }));
            } else if matches!(evidence, Evidence::Found(_)) {
                assert!(matches!(next, Action::EnqueueCompletion(Ack::Confirmed)));
            } else {
                assert!(matches!(next, Action::Observe(_)));
            }
        }
    }
}

#[test]
fn sent_recovers_to_uncertain_without_fresh_dispatch() {
    assert!(matches!(action(view(State::Sent), Input::Start), Action::Transition(State::Uncertain)));
}

#[test]
fn quota_is_checked_after_observing_and_retirement_skips_observing() {
    let v = DeliveryView { waits: Waits { quota: true, ..view(State::Uncertain).waits }, ..view(State::Uncertain) };
    let Action::Observe(pending) = action(v, Input::Start) else { panic!("observe before quota wait") };
    assert!(matches!(action(v, Input::Observed { pending, revision_changed: false }), Action::Wait));
    assert!(matches!(action(DeliveryView { state: State::Cancelling, fence: Fence::Retiring, ..v }, Input::Start), Action::Wait));
}

#[test]
fn reconciliation_does_not_treat_pending_or_error_as_absence() {
    for state in [State::Uncertain, State::Cancelling] {
        for held in [false, true] {
            let v = DeliveryView { waits: Waits { held, ..view(state).waits }, ..view(state) };
            assert!(matches!(reconcile(v, Evidence::Found(Ack::Confirmed)), Action::EnqueueCompletion(Ack::Confirmed)));
            assert!(matches!(reconcile(v, Evidence::Pending), Action::Wait));
            assert!(matches!(reconcile(v, Evidence::Error), Action::Wait));
            let result = reconcile(v, Evidence::Absent);
            match (state, held) {
                (State::Cancelling, _) => assert!(matches!(result, Action::CancelAbsent)),
                (_, false) => assert!(matches!(result, Action::Transition(State::Intended))),
                (_, true) => assert!(matches!(result, Action::Wait)),
            }
        }
    }
}

#[test]
fn approval_pending_is_ambiguous_only_when_live_and_unheld() {
    for held in [false, true] {
        for fence in [Fence::Live, Fence::Retiring] {
            let v = DeliveryView { state: if fence == Fence::Live { State::Uncertain } else { State::Cancelling }, approval: ApprovalGate::Fresh, fence, waits: Waits { held, ..view(State::Uncertain).waits }, ..view(State::Uncertain) };
            let next = reconcile(v, Evidence::Pending);
            if !held && fence == Fence::Live { assert!(matches!(next, Action::RecordAmbiguity { stop: false })); }
            else { assert!(matches!(next, Action::Wait)); }
        }
    }
}

#[test]
fn completion_bridges_uncertain_and_retires_only_kernel_retries() {
    for state in [State::Sent, State::Uncertain, State::Cancelling] {
        for ack in [Ack::Confirmed, Ack::Retryable, Ack::Permanent, Ack::BodyPolicyRefused] {
            for kind in [DeliveryKind::KernelEffect, DeliveryKind::Auxiliary] {
                let v = DeliveryView { kind, ..view(state) };
                let Action::Commit(plan) = action(v, Input::Consume { key: b"original", ack, kernel_ack: Ack::BodyPolicyRefused, approval_successor: true }) else { panic!() };
                assert_eq!(plan.completion, completion(state, ack));
                let retire = kind == DeliveryKind::KernelEffect && ack == Ack::Retryable && state != State::Cancelling;
                assert_eq!(plan.retire_retry, retire);
                assert_eq!(plan.remove_watch, retire);
                assert!(!plan.authorize_successor);
            }
        }
    }
    assert!(matches!(action(view(State::Sent), Input::Consume { key: b"other", ack: Ack::Confirmed, kernel_ack: Ack::Confirmed, approval_successor: false }), Action::Wait));
}

#[test]
fn approval_clear_news_and_repair_preserve_q27() {
    let v = DeliveryView { approval: ApprovalGate::Prior { predecessor: false }, ..view(State::Uncertain) };
    assert!(matches!(action(v, Input::ClearApproval { key: b"original", matching_open_incident: true }), Action::AuthorizeRetry));
    assert!(matches!(action(v, Input::ClearApproval { key: b"original", matching_open_incident: false }), Action::Wait));
    let final_v = DeliveryView { state: State::Confirmed, ..v };
    assert!(matches!(action(final_v, Input::CheckedApprovalNews { key: b"original" }), Action::ScheduleRepair));
    assert!(matches!(action(final_v, Input::ClearApproval { key: b"original", matching_open_incident: true }), Action::Wait));
    for copies in [None, Some(1), Some(2)] {
        assert!(matches!(action(final_v, Input::RepairDone { copies, dispatches: 2 }), Action::FinishRepair { clear_unresolved } if clear_unresolved == (copies == Some(2))));
    }
}

#[test]
fn continuations_cannot_cross_key_epoch_or_revision() {
    let (v, pending) = sent(view(State::Intended));
    let Action::Observe(pending) = action(v, Input::SentCommitted(pending)) else { panic!() };
    assert!(matches!(action(DeliveryView { key: b"replacement", ..v }, Input::Observed { pending, revision_changed: false }), Action::Wait));
    let (v, pending) = sent(view(State::Intended));
    assert!(matches!(action(DeliveryView { epoch: 4, ..v }, Input::SentCommitted(pending)), Action::Wait));
    let (v, pending) = sent(view(State::Intended));
    let Action::Observe(pending) = action(v, Input::SentCommitted(pending)) else { panic!() };
    assert!(matches!(action(DeliveryView { revision: 12, ..v }, Input::Observed { pending, revision_changed: false }), Action::Wait));
}
