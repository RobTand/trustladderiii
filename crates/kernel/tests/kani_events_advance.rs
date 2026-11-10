//! The same assertions on enumerated and sampled inputs.
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;
#[path = "kani_scenarios/common/a_extra.rs"]
mod a_extra;

#[test]
fn kani_apply_new_request() {
    a_extra::run_event("kani_apply_new_request", "new_request");
}

#[test]
fn kani_apply_ingest_done() {
    a_extra::run_event("kani_apply_ingest_done", "ingest_done");
}

#[test]
fn kani_apply_ingest_early_end() {
    a_extra::run_event("kani_apply_ingest_early_end", "ingest_early_end");
}

#[test]
fn kani_apply_protected_paths() {
    a_extra::run_event("kani_apply_protected_paths", "protected_paths");
}

#[test]
fn kani_apply_audit_agrees() {
    a_extra::run_event("kani_apply_audit_agrees", "audit_agrees");
}

#[test]
fn kani_apply_plan_leaf() {
    a_extra::run_event("kani_apply_plan_leaf", "plan_leaf");
}

#[test]
fn kani_apply_plan_split() {
    a_extra::run_event("kani_apply_plan_split", "plan_split");
}

#[test]
fn kani_apply_plan_cant_meet() {
    a_extra::run_event("kani_apply_plan_cant_meet", "plan_cant_meet");
}

#[test]
fn kani_apply_maker_done() {
    a_extra::run_event("kani_apply_maker_done", "maker_done");
}

#[test]
fn kani_apply_maker_needs_files() {
    a_extra::run_event("kani_apply_maker_needs_files", "maker_needs_files");
}

#[test]
fn kani_apply_maker_cant_meet() {
    a_extra::run_event("kani_apply_maker_cant_meet", "maker_cant_meet");
}

#[test]
fn kani_apply_maker_out_of_room() {
    a_extra::run_event("kani_apply_maker_out_of_room", "maker_out_of_room");
}

#[test]
fn kani_apply_checks_done() {
    a_extra::run_event("kani_apply_checks_done", "checks_done");
}

#[test]
fn kani_apply_approved() {
    a_extra::run_event("kani_apply_approved", "approved");
}

#[test]
fn kani_apply_reviewer_out_of_room() {
    a_extra::run_event("kani_apply_reviewer_out_of_room", "reviewer_out_of_room");
}

#[test]
fn kani_apply_changes_requested() {
    a_extra::run_event("kani_apply_changes_requested", "changes_requested");
}

#[test]
fn kani_apply_rebase_clean() {
    a_extra::run_event("kani_apply_rebase_clean", "rebase_clean");
}

#[test]
fn kani_apply_rebase_needs_changes() {
    a_extra::run_event("kani_apply_rebase_needs_changes", "rebase_needs_changes");
}

#[test]
fn kani_apply_run_failed() {
    a_extra::run_event("kani_apply_run_failed", "run_failed");
}

#[test]
fn kani_apply_ack_confirmed() {
    a_extra::run_event("kani_apply_ack_confirmed", "ack_confirmed");
}

#[test]
fn kani_apply_ack_delivery_accepted() {
    a_extra::run_event("kani_apply_ack_delivery_accepted", "ack_delivery_accepted");
}

#[test]
fn kani_apply_ack_retryable() {
    a_extra::run_event("kani_apply_ack_retryable", "ack_retryable");
}

#[test]
fn kani_apply_ack_permanent() {
    a_extra::run_event("kani_apply_ack_permanent", "ack_permanent");
}

#[test]
fn kani_apply_ack_body_policy_refused() {
    a_extra::run_event("kani_apply_ack_body_policy_refused", "ack_body_policy_refused");
}

#[test]
fn kani_apply_outside_push() {
    a_extra::run_event("kani_apply_outside_push", "outside_push");
}

#[test]
fn kani_apply_ci_failed() {
    a_extra::run_event("kani_apply_ci_failed", "ci_failed");
}

#[test]
fn kani_apply_stop() {
    a_extra::run_event("kani_apply_stop", "stop");
}

#[test]
fn kani_apply_pause() {
    a_extra::run_event("kani_apply_pause", "pause");
}

#[test]
fn kani_apply_resume() {
    a_extra::run_event("kani_apply_resume", "resume");
}

#[test]
fn kani_apply_change_request() {
    a_extra::run_event("kani_apply_change_request", "change_request");
}

#[test]
fn kani_apply_answer_continue() {
    a_extra::run_event("kani_apply_answer_continue", "answer_continue");
}

#[test]
fn kani_apply_answer_try_again() {
    a_extra::run_event("kani_apply_answer_try_again", "answer_try_again");
}

#[test]
fn kani_apply_answer_add_credit() {
    a_extra::run_event("kani_apply_answer_add_credit", "answer_add_credit");
}

#[test]
fn kani_apply_answer_change_request() {
    a_extra::run_event("kani_apply_answer_change_request", "answer_change_request");
}

#[test]
fn kani_apply_answer_stop() {
    a_extra::run_event("kani_apply_answer_stop", "answer_stop");
}

#[test]
fn kani_apply_answer_unclear() {
    a_extra::run_event("kani_apply_answer_unclear", "answer_unclear");
}

#[test]
fn kani_apply_answer_retry() {
    a_extra::run_event("kani_apply_answer_retry", "answer_retry");
}

#[test]
fn kani_apply_answer_mark_handled() {
    a_extra::run_event("kani_apply_answer_mark_handled", "answer_mark_handled");
}

#[test]
fn kani_advance_ingesting() {
    a_extra::run_advance("kani_advance_ingesting", &[Phase::Ingesting], true);
}

#[test]
fn kani_advance_auditing() {
    a_extra::run_advance("kani_advance_auditing", &[Phase::Auditing], true);
}

#[test]
fn kani_advance_unplanned() {
    a_extra::run_advance("kani_advance_unplanned", &[Phase::Unplanned], true);
}

#[test]
fn kani_advance_planning() {
    a_extra::run_advance("kani_advance_planning", &[Phase::Planning { must_split: false }, Phase::Planning { must_split: true }], true);
}

#[test]
fn kani_advance_ready() {
    a_extra::run_advance("kani_advance_ready", &[Phase::Ready(Resume::Making), Phase::Ready(Resume::Checking), Phase::Ready(Resume::Reviewing), Phase::Ready(Resume::Publishing), Phase::Ready(Resume::Rebasing)], true);
}

#[test]
fn kani_advance_making() {
    a_extra::run_advance("kani_advance_making", &[Phase::Making], true);
}

#[test]
fn kani_advance_checking() {
    a_extra::run_advance("kani_advance_checking", &[Phase::Checking], true);
}

#[test]
fn kani_advance_reviewing() {
    a_extra::run_advance("kani_advance_reviewing", &[Phase::Reviewing], true);
}

#[test]
fn kani_advance_publishing() {
    a_extra::run_advance("kani_advance_publishing", &[Phase::Publishing], true);
}

#[test]
fn kani_advance_awaiting_delivery() {
    a_extra::run_advance("kani_advance_awaiting_delivery", &[Phase::AwaitingDelivery], false);
}

#[test]
fn kani_advance_rebasing() {
    a_extra::run_advance("kani_advance_rebasing", &[Phase::Rebasing], true);
}

#[test]
fn kani_advance_stopping() {
    a_extra::run_advance("kani_advance_stopping", &[Phase::Stopping(Outcome::Merged), Phase::Stopping(Outcome::Stopped), Phase::Stopping(Outcome::Replaced), Phase::Stopping(Outcome::Duplicate), Phase::Stopping(Outcome::AlreadyDone), Phase::Stopping(Outcome::NotActionable)], true);
}

#[test]
fn kani_apply_new_adoption() {
    a_extra::run_event("kani_apply_new_adoption", "new_adoption");
}
