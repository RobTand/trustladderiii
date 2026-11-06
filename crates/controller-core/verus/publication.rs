#[path = "../src/publication.rs"]
mod publication;
use publication::*;
use vstd::prelude::*;

verus! {
/// Escape: state-table.md:728-735, :744, :751, :755 at cdacce8f are effect/fence context.
/// Route precedence preserves github.rs:636-704 at cdacce8f (no rule).
/// Concrete calls reach every route, including publication precedence over steering.
fn route_witnesses() {
    let witness1 = reconcile_route(Output::Approval, Steering::Correction);
    assert(witness1 == ReconcileRoute::Approval);
    let witness2 = reconcile_route(Output::Publication, Steering::Restart);
    assert(witness2 == ReconcileRoute::Publication);
    let witness3 = reconcile_route(Output::Other, Steering::Correction);
    assert(witness3 == ReconcileRoute::Issue);
    let witness4 = reconcile_route(Output::Other, Steering::Restart);
    assert(witness4 == ReconcileRoute::Restart);
    let witness5 = reconcile_route(Output::Other, Steering::ConsoleNote);
    assert(witness5 == ReconcileRoute::ConsoleNote);
    let witness6 = reconcile_route(Output::Message, Steering::Other);
    assert(witness6 == ReconcileRoute::Message);
    let witness7 = reconcile_route(Output::Other, Steering::Other);
    assert(witness7 == ReconcileRoute::Records);
}
/// Escape: preserved existing behaviour (no rule): github.rs:640-651 at cdacce8f;
/// ee9e40cfe021 introduced pr_evidence -> Found(permanent).
fn permanence_witnesses() {
    let witness8 = publication_reconciliation(PublicationRead::EvidencePermanent);
    assert(witness8 == ReconcileAction::FoundPermanent);
    let witness9 = publication_reconciliation(PublicationRead::OtherPermanent);
    assert(witness9 == ReconcileAction::Propagate);
    let witness10 = publication_reconciliation(PublicationRead::Failed);
    assert(witness10 == ReconcileAction::Pending);
    let witness11 = publication_reconciliation(PublicationRead::Reconciled);
    assert(witness11 == ReconcileAction::Preserve);
}
/// Escape: state-table.md:746 at cdacce8f is PR reconciliation context.
/// Exact acceptance preserves publishing.rs:393-408 at cdacce8f (no rule).
/// Each conjunct independently prevents acceptance; no precondition excludes rejection.
fn acceptance_witnesses() {
    let witness12 = accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:true, no_gaps:true, evidence_idempotent:true});
    assert(witness12);
    let witness13 = accept_open_pr(OpenPrEvidence {open_or_merged:false, valid_body:true, valid_obligations:true, no_gaps:true, evidence_idempotent:true});
    assert(!witness13);
    let witness14 = accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:false, valid_obligations:true, no_gaps:true, evidence_idempotent:true});
    assert(!witness14);
    let witness15 = accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:false, no_gaps:true, evidence_idempotent:true});
    assert(!witness15);
    let witness16 = accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:true, no_gaps:false, evidence_idempotent:true});
    assert(!witness16);
    let witness17 = accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:true, no_gaps:true, evidence_idempotent:false});
    assert(!witness17);
}
/// Escape: state-table.md:744 at cdacce8f; explicit #766 decision in
/// state-machine.md:963-969 at cdacce8f; #769 duplicate convergence.
fn preflight_witnesses() {
    let witness18 = approval_preflight(true);
    assert(witness18 == ApprovalPreflight::Observed);
    let witness19 = approval_preflight(false);
    assert(witness19 == ApprovalPreflight::Dispatch);
}
/// Escape: state-table.md:728, :744, :753 at cdacce8f; explicit #766 decision
/// in state-machine.md:963-969; #769. Exact failure split preserves
/// publishing.rs:201-224 at cdacce8f (no additional rule).
fn outcome_witnesses() {
    let witness20 = failed_approval(ApprovalFailure {sent:false,permanent:false});
    assert(witness20 == ApprovalFailureAction::Refused);
    let witness21 = failed_approval(ApprovalFailure {sent:false,permanent:true});
    assert(witness21 == ApprovalFailureAction::Refused);
    let witness22 = failed_approval(ApprovalFailure {sent:true,permanent:true});
    assert(witness22 == ApprovalFailureAction::Permanent);
    let witness23 = failed_approval(ApprovalFailure {sent:true,permanent:false});
    assert(witness23 == ApprovalFailureAction::Uncertain);
}
/// Escape: state-table.md:744 at cdacce8f; explicit #766 decision in
/// state-machine.md:963-969. Exact evidence check preserves
/// publishing.rs:225-230 at cdacce8f (no additional rule).
fn observation_witnesses() {
    let witness24 = observed_approval(true);
    assert(witness24 == ApprovalObservation::Observed);
    let witness25 = observed_approval(false);
    assert(witness25 == ApprovalObservation::Uncertain);
}
/// Escape: state-table.md:751 at cdacce8f is comment-delivery context.
/// Eligibility preserves github.rs:694-704, :1012-1024 at cdacce8f (no rule).
fn message_witnesses() {
    let witness26 = message_delivery(false, Comment::Clarify);
    assert(witness26);
    let witness27 = message_delivery(false, Comment::ReplacementDropped);
    assert(witness27);
    let witness28 = message_delivery(false, Comment::Other);
    assert(!witness28);
    let witness29 = message_delivery(true, Comment::Other);
    assert(witness29);
}
/// Escape: state-table.md:915 and Q17 (state-machine.md:502) at cdacce8f
/// require the operations issue. Exact identity check preserves
/// github.rs:1025-1036 at cdacce8f (no additional rule).
fn incident_witnesses() {
    let witness30 = incident_admitted(true,None);
    assert(!witness30);
    let witness31 = incident_admitted(false,None);
    assert(witness31);
    let witness32 = incident_admitted(true,Some(OperationsTarget {owner:true,repository:true,installation:true,issue:true}));
    assert(witness32);
    let witness33 = incident_admitted(true,Some(OperationsTarget {owner:false,repository:true,installation:true,issue:true}));
    assert(!witness33);
    let witness34 = incident_admitted(true,Some(OperationsTarget {owner:true,repository:false,installation:true,issue:true}));
    assert(!witness34);
    let witness35 = incident_admitted(true,Some(OperationsTarget {owner:true,repository:true,installation:false,issue:true}));
    assert(!witness35);
    let witness36 = incident_admitted(true,Some(OperationsTarget {owner:true,repository:true,installation:true,issue:false}));
    assert(!witness36);
}
/// Escape: state-table.md:678, :915; Q17/Q49 (state-machine.md:502/:479)
/// at cdacce8f are incident/restart context. Exact title-source predicate
/// preserves github.rs:1046-1052 at cdacce8f (no additional rule).
fn title_witnesses() {
    let witness37 = title_source(false,false);
    assert(witness37 == TitleSource::Issue);
    let witness38 = title_source(true,false);
    assert(witness38 == TitleSource::Context);
    let witness39 = title_source(false,true);
    assert(witness39 == TitleSource::Context);
    let witness40 = title_source(true,true);
    assert(witness40 == TitleSource::Context);
}
/// Escape: preserved existing behaviour (no rule): github.rs:1097-1114 at
/// cdacce8f; 60e70bc7d280 introduced the Clarify-only rephrase fallback.
fn rejection_witnesses() {
    let witness41 = rejected_message(Comment::Clarify);
    assert(witness41 == MessageRejection::Fallback);
    let witness42 = rejected_message(Comment::ReplacementDropped);
    assert(witness42 == MessageRejection::Permanent);
    let witness43 = rejected_message(Comment::Other);
    assert(witness43 == MessageRejection::Permanent);
}
/// Escape: state-table.md:696, :728-730; Q49 (state-machine.md:479) at
/// cdacce8f are closure/reconciliation context. Exact read/close split
/// preserves github.rs:714-731 at cdacce8f (no additional rule).
fn close_witnesses() {
    let witness44 = close_after_reconcile(IssueRead::Closed);
    assert(witness44 == CloseAction::Complete);
    let witness45 = close_after_reconcile(IssueRead::Open);
    assert(witness45 == CloseAction::Close);
    let witness46 = close_after_reconcile(IssueRead::Failed);
    assert(witness46 == CloseAction::Pending);
}
}
fn main() {}
