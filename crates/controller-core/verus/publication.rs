#[path = "../src/publication.rs"]
mod publication;
use publication::*;
use vstd::prelude::*;

verus! {
/// Escape: state-table.md:728-735, :755 at cdacce8f; Q27/Q49, #769/#766.
/// Concrete calls reach every route, including publication precedence over steering.
fn route_witnesses() {
    assert(reconcile_route(Output::Approval, Steering::Correction) == ReconcileRoute::Approval);
    assert(reconcile_route(Output::Publication, Steering::Restart) == ReconcileRoute::Publication);
    assert(reconcile_route(Output::Other, Steering::Correction) == ReconcileRoute::Issue);
    assert(reconcile_route(Output::Other, Steering::Restart) == ReconcileRoute::Restart);
    assert(reconcile_route(Output::Other, Steering::ConsoleNote) == ReconcileRoute::ConsoleNote);
    assert(reconcile_route(Output::Message, Steering::Other) == ReconcileRoute::Message);
    assert(reconcile_route(Output::Other, Steering::Other) == ReconcileRoute::Records);
}
/// Escape: state-table.md:727-730 at cdacce8f; Q27/Q54, #769/#766.
fn permanence_witnesses() {
    assert(publication_reconciliation(PublicationRead::EvidencePermanent) == ReconcileAction::FoundPermanent);
    assert(publication_reconciliation(PublicationRead::OtherPermanent) == ReconcileAction::Propagate);
    assert(publication_reconciliation(PublicationRead::Failed) == ReconcileAction::Pending);
    assert(publication_reconciliation(PublicationRead::Reconciled) == ReconcileAction::Preserve);
}
/// Escape: state-table.md:728-730, :745 at cdacce8f; Q27/Q45/Q52, #769/#766.
/// Each conjunct independently prevents acceptance; no precondition excludes rejection.
fn acceptance_witnesses() {
    assert(accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:true, no_gaps:true, evidence_idempotent:true}));
    assert(!accept_open_pr(OpenPrEvidence {open_or_merged:false, valid_body:true, valid_obligations:true, no_gaps:true, evidence_idempotent:true}));
    assert(!accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:false, valid_obligations:true, no_gaps:true, evidence_idempotent:true}));
    assert(!accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:false, no_gaps:true, evidence_idempotent:true}));
    assert(!accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:true, no_gaps:false, evidence_idempotent:true}));
    assert(!accept_open_pr(OpenPrEvidence {open_or_merged:true, valid_body:true, valid_obligations:true, no_gaps:true, evidence_idempotent:false}));
}
/// Escape: state-table.md:743 at cdacce8f; Q27, #769/#766.
fn preflight_witnesses() {
    assert(approval_preflight(true) == ApprovalPreflight::Observed);
    assert(approval_preflight(false) == ApprovalPreflight::Dispatch);
}
/// Escape: state-table.md:727-730, :743, :753 at cdacce8f; Q27, #769/#766.
fn outcome_witnesses() {
    assert(failed_approval(ApprovalFailure {sent:false,permanent:false}) == ApprovalFailureAction::Refused);
    assert(failed_approval(ApprovalFailure {sent:false,permanent:true}) == ApprovalFailureAction::Refused);
    assert(failed_approval(ApprovalFailure {sent:true,permanent:true}) == ApprovalFailureAction::Permanent);
    assert(failed_approval(ApprovalFailure {sent:true,permanent:false}) == ApprovalFailureAction::Uncertain);
}
/// Escape: state-table.md:728-730, :743 at cdacce8f; Q27, #769/#766.
fn observation_witnesses() {
    assert(observed_approval(true) == ApprovalObservation::Observed);
    assert(observed_approval(false) == ApprovalObservation::Uncertain);
}
/// Escape: state-table.md:751 at cdacce8f; Q27/Q49, #769/#766.
fn message_witnesses() {
    assert(message_delivery(false, Comment::Clarify));
    assert(message_delivery(false, Comment::ReplacementDropped));
    assert(!message_delivery(false, Comment::Other));
    assert(message_delivery(true, Comment::Other));
}
/// Escape: state-table.md:915 at cdacce8f; Q17/Q27, #769/#766.
fn incident_witnesses() {
    assert(!incident_admitted(true,None));
    assert(incident_admitted(false,None));
    assert(incident_admitted(true,Some(OperationsTarget {owner:true,repository:true,installation:true,issue:true})));
    assert(!incident_admitted(true,Some(OperationsTarget {owner:false,repository:true,installation:true,issue:true})));
    assert(!incident_admitted(true,Some(OperationsTarget {owner:true,repository:false,installation:true,issue:true})));
    assert(!incident_admitted(true,Some(OperationsTarget {owner:true,repository:true,installation:false,issue:true})));
    assert(!incident_admitted(true,Some(OperationsTarget {owner:true,repository:true,installation:true,issue:false})));
}
/// Escape: state-table.md:678, :915 at cdacce8f; Q17/Q27/Q49, #769/#766.
fn title_witnesses() {
    assert(title_source(false,false) == TitleSource::Issue);
    assert(title_source(true,false) == TitleSource::Context);
    assert(title_source(false,true) == TitleSource::Context);
    assert(title_source(true,true) == TitleSource::Context);
}
/// Escape: state-table.md:727, :751 at cdacce8f; Q27/Q54, #769/#766.
fn rejection_witnesses() {
    assert(rejected_message(Comment::Clarify) == MessageRejection::Fallback);
    assert(rejected_message(Comment::ReplacementDropped) == MessageRejection::Permanent);
    assert(rejected_message(Comment::Other) == MessageRejection::Permanent);
}
/// Escape: state-table.md:696, :728-730 at cdacce8f; Q27/Q49, #769/#766.
fn close_witnesses() {
    assert(close_after_reconcile(IssueRead::Closed) == CloseAction::Complete);
    assert(close_after_reconcile(IssueRead::Open) == CloseAction::Close);
    assert(close_after_reconcile(IssueRead::Failed) == CloseAction::Pending);
}
}
fn main() {}
