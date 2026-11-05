use tl3_controller::dispatch_liveness::decision::{
    DeliveryClass, DispatchStatus, Evidence, ProcessEvidence, classify,
};
use tl3_kernel::{EffectKind, Role, Wait};

fn evidence() -> Evidence {
    Evidence {
        class: DeliveryClass::Worker,
        intent: false,
        watch: false,
        process: ProcessEvidence::Unknown,
        ending: false,
        deadline: false,
        quota_reset: None,
        incident: false,
        paused: false,
        ticket: false,
        obstruction: false,
        question: false,
        external: false,
        closed: false,
        admission: false,
    }
}

/// #817 / effect delivery classes: only worker dispatch needs a live process.
#[test]
fn delivery_contracts_and_current_endings_are_distinct() {
    let mut e = evidence();
    e.intent = true;
    e.watch = true;
    e.process = ProcessEvidence::Live;
    assert_eq!(
        classify(Wait::WorkerReply, e),
        DispatchStatus::Dispatched(DeliveryClass::Worker)
    );
    e.process = ProcessEvidence::Ended;
    assert_eq!(classify(Wait::WorkerReply, e), DispatchStatus::Silent);
    e.ending = true;
    assert_eq!(classify(Wait::WorkerReply, e), DispatchStatus::Ending);
    e.ending = false;
    for class in [
        DeliveryClass::Once,
        DeliveryClass::Converges,
        DeliveryClass::AtLeastOnce,
    ] {
        e.class = class;
        assert_eq!(
            classify(
                Wait::Effect(match class {
                    DeliveryClass::Once => EffectKind::SubmitChecks,
                    DeliveryClass::Converges => EffectKind::Push,
                    DeliveryClass::AtLeastOnce => EffectKind::PostApproval,
                    DeliveryClass::Worker => unreachable!(),
                }),
                e
            ),
            DispatchStatus::Dispatched(class)
        );
    }
}

/// Q39: a pool occupant's silence invalidates the obstruction certificate.
#[test]
fn pool_obstruction_requires_covered_occupants_or_an_earlier_ticket() {
    let mut e = evidence();
    e.ticket = true;
    assert_eq!(classify(Wait::AgentPool, e), DispatchStatus::Silent);
    e.obstruction = true;
    assert_eq!(classify(Wait::AgentPool, e), DispatchStatus::PoolWait);
    e.paused = true;
    assert_eq!(classify(Wait::Resume, e), DispatchStatus::Paused);
}

/// State-table question, queue, gate, prerequisites, Closed and #771 halt rows.
#[test]
fn every_other_wait_has_a_typed_evidence_requirement() {
    let mut e = evidence();
    e.question = true;
    assert_eq!(classify(Wait::Answer, e), DispatchStatus::Answer);
    e.external = true;
    assert_eq!(classify(Wait::MergeQueue, e), DispatchStatus::MergeQueue);
    e.ticket = true;
    e.obstruction = true;
    assert_eq!(classify(Wait::SlotOrFiles, e), DispatchStatus::SlotOrFiles);
    assert_eq!(
        classify(Wait::UpstreamEvent, e),
        DispatchStatus::Prerequisite
    );
    e.closed = true;
    assert_eq!(classify(Wait::None, e), DispatchStatus::Closed);
    e.incident = true;
    for wait in [
        Wait::Incident,
        Wait::WorkerReply,
        Wait::CheckResults,
        Wait::Effect(EffectKind::StartRun(Role::Audit)),
    ] {
        assert_eq!(classify(wait, e), DispatchStatus::Incident);
    }
}

/// #817 review: an outbox intent is not delivery-class contract evidence.
#[test]
fn immutable_intent_alone_never_certifies_non_worker_dispatch() {
    let mut e = evidence();
    e.intent = true;
    for kind in [
        EffectKind::SubmitChecks,
        EffectKind::StartRebase,
        EffectKind::PrepareWorktree,
        EffectKind::Push,
        EffectKind::OpenPr,
        EffectKind::Enqueue,
        EffectKind::WithdrawJob,
        EffectKind::ClosePr,
        EffectKind::RemoveWorktree,
        EffectKind::PostApproval,
    ] {
        assert_eq!(
            classify(Wait::Effect(kind), e),
            DispatchStatus::Silent,
            "{kind:?}"
        );
        e.deadline = true;
        assert_eq!(
            classify(Wait::Effect(kind), e),
            DispatchStatus::Retry(
                tl3_controller::dispatch_liveness::decision::RetryPending::Allowance
            )
        );
        e.deadline = false;
    }
}
