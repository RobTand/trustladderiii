use vstd::prelude::*;
use crate::worker_events::*;
verus! {
/// State-table cdacce8f :277,787; Q53/#759. Witness explicit valid charge and
/// forbidden non-postmortem/wrong-model charges, plus the legacy implicit case.
fn model_witnesses() {
    let out_1 = quota_model(true, ModelField::GradeModel);
    assert(out_1 == QuotaModel::Grade);
    let out_2 = quota_model(false, ModelField::GradeModel);
    assert(out_2 == QuotaModel::Forbidden);
    let out_3 = quota_model(true, ModelField::Other);
    assert(out_3 == QuotaModel::Forbidden);
    let out_4 = quota_model(true, ModelField::Missing);
    assert(out_4 == QuotaModel::Implicit);
}
/// State-table cdacce8f :277,787,800; Q43/Q53, #759/#682/#691/#696.
/// Every sequence outcome has a concrete reachable input; no stream-length bound.
fn sequence_witnesses() {
    let quota = WorkerEvent::Quota { model: QuotaModel::Grade, reset_ms: Some(1) };
    let out_5 = reply_step(quota, false, true);
    assert(out_5 == ReplyDecision::Quota);
    let out_6 = reply_step(quota, false, false);
    assert(out_6 == ReplyDecision::AfterQuota);
    let out_7 = reply_step(quota, true, true);
    assert(out_7 == ReplyDecision::AfterReply);
    let out_8 = reply_step(WorkerEvent::Invalid, false, true);
    assert(out_8 == ReplyDecision::InvalidEvent);
    let out_9 = reply_step(WorkerEvent::Reply, false, true);
    assert(out_9 == ReplyDecision::Reply);
    let out_10 = reply_step(WorkerEvent::Progress { unit: ProgressUnit::ToolResult }, false, true);
    assert(out_10 == ReplyDecision::Progress);
    let out_11 = reply_step(WorkerEvent::Quota { model: QuotaModel::Forbidden, reset_ms: Some(1) }, false, true);
    assert(out_11 == ReplyDecision::InvalidQuotaModel);
    let out_12 = reply_step(WorkerEvent::Quota { model: QuotaModel::Implicit, reset_ms: None }, false, true);
    assert(out_12 == ReplyDecision::InvalidQuotaReset);
    let out_13 = reply_step(WorkerEvent::Quota { model: QuotaModel::Implicit, reset_ms: Some(0) }, false, true);
    assert(out_13 == ReplyDecision::InvalidQuotaReset);
}
/// State-table cdacce8f :147,800; Q43, #682/#691/#696. Full-width gate witnesses.
fn progress_witnesses() {
    let event = WorkerEvent::Progress { unit: ProgressUnit::ExternalWait };
    let out_14 = progress_due(event, 1, 0);
    assert(out_14);
    let out_15 = progress_due(event, 1, 1);
    assert(!out_15);
    let out_16 = progress_due(event, 0, 1);
    assert(!out_16);
    let out_17 = progress_due(WorkerEvent::Reply, 1, 0);
    assert(!out_17);
    let out_18 = progress_signal(true, true);
    assert(out_18 == ProgressSignal::Look);
    let out_19 = progress_signal(true, false);
    assert(out_19 == ProgressSignal::None);
    let out_20 = progress_signal(false, true);
    assert(out_20 == ProgressSignal::Progress);
    let out_21 = progress_signal(false, false);
    assert(out_21 == ProgressSignal::Progress);
}
/// State-table cdacce8f :143,800,812; Q43, #682/#691/#696.
fn lifecycle_witnesses() {
    let out_22 = refresh_gate(false);
    assert(out_22 == RefreshGate::Retire);
    let out_23 = refresh_gate(true);
    assert(out_23 == RefreshGate::Inspect);
    let out_24 = claim_decision(Claim::Missing);
    assert(out_24 == ClaimDecision::AwaitClaim);
    let out_25 = claim_decision(Claim::Cancelled);
    assert(out_25 == ClaimDecision::Retire);
    let out_26 = claim_decision(Claim::Live);
    assert(out_26 == ClaimDecision::ReadOutput);
    let out_27 = end_decision(Ending::Found);
    assert(out_27 == EndDecision::Complete);
    let out_28 = end_decision(Ending::Absent);
    assert(out_28 == EndDecision::Retire);
    let out_29 = end_decision(Ending::Pending);
    assert(out_29 == EndDecision::KeepActive);
}
}
