//! The same assertions on enumerated and sampled inputs.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;
#[path = "kani_scenarios/common/a_extra.rs"]
mod a_extra;

#[test]
fn kani_apply_new_request() {
    a_extra::run_event(6, "kani_apply_new_request");
}

#[test]
fn kani_apply_ingest_done() {
    a_extra::run_event(7, "kani_apply_ingest_done");
}

#[test]
fn kani_apply_ingest_early_end() {
    a_extra::run_event(8, "kani_apply_ingest_early_end");
}

#[test]
fn kani_apply_protected_paths() {
    a_extra::run_event(9, "kani_apply_protected_paths");
}

#[test]
fn kani_apply_audit_agrees() {
    a_extra::run_event(10, "kani_apply_audit_agrees");
}

#[test]
fn kani_apply_plan_leaf() {
    a_extra::run_event(11, "kani_apply_plan_leaf");
}

#[test]
fn kani_apply_plan_split() {
    a_extra::run_event(12, "kani_apply_plan_split");
}

#[test]
fn kani_apply_plan_cant_meet() {
    a_extra::run_event(13, "kani_apply_plan_cant_meet");
}

#[test]
fn kani_apply_maker_done() {
    a_extra::run_event(14, "kani_apply_maker_done");
}

#[test]
fn kani_apply_maker_needs_files() {
    a_extra::run_event(15, "kani_apply_maker_needs_files");
}

#[test]
fn kani_apply_maker_cant_meet() {
    a_extra::run_event(16, "kani_apply_maker_cant_meet");
}

#[test]
fn kani_apply_maker_out_of_room() {
    a_extra::run_event(17, "kani_apply_maker_out_of_room");
}

#[test]
fn kani_apply_checks_done() {
    a_extra::run_event(18, "kani_apply_checks_done");
}

#[test]
fn kani_apply_approved() {
    a_extra::run_event(19, "kani_apply_approved");
}

#[test]
fn kani_apply_reviewer_out_of_room() {
    a_extra::run_event(20, "kani_apply_reviewer_out_of_room");
}

#[test]
fn kani_apply_changes_requested() {
    a_extra::run_event(21, "kani_apply_changes_requested");
}

#[test]
fn kani_apply_rebase_clean() {
    a_extra::run_event(22, "kani_apply_rebase_clean");
}

#[test]
fn kani_apply_rebase_needs_changes() {
    a_extra::run_event(23, "kani_apply_rebase_needs_changes");
}

#[test]
fn kani_apply_run_failed() {
    a_extra::run_event(24, "kani_apply_run_failed");
}

#[test]
fn kani_apply_ack_confirmed() {
    a_extra::run_event(25, "kani_apply_ack_confirmed");
}

#[test]
fn kani_apply_ack_delivery_accepted() {
    a_extra::run_event(26, "kani_apply_ack_delivery_accepted");
}

#[test]
fn kani_apply_ack_retryable() {
    a_extra::run_event(27, "kani_apply_ack_retryable");
}

#[test]
fn kani_apply_ack_permanent() {
    a_extra::run_event(28, "kani_apply_ack_permanent");
}

#[test]
fn kani_apply_ack_body_policy_refused() {
    a_extra::run_event(29, "kani_apply_ack_body_policy_refused");
}

#[test]
fn kani_apply_outside_push() {
    a_extra::run_event(30, "kani_apply_outside_push");
}

#[test]
fn kani_apply_ci_failed() {
    a_extra::run_event(31, "kani_apply_ci_failed");
}

#[test]
fn kani_apply_stop() {
    a_extra::run_event(32, "kani_apply_stop");
}

#[test]
fn kani_apply_pause() {
    a_extra::run_event(33, "kani_apply_pause");
}

#[test]
fn kani_apply_resume() {
    a_extra::run_event(34, "kani_apply_resume");
}

#[test]
fn kani_apply_change_request() {
    a_extra::run_event(35, "kani_apply_change_request");
}

#[test]
fn kani_apply_answer_continue() {
    a_extra::run_event(36, "kani_apply_answer_continue");
}

#[test]
fn kani_apply_answer_try_again() {
    a_extra::run_event(37, "kani_apply_answer_try_again");
}

#[test]
fn kani_apply_answer_add_credit() {
    a_extra::run_event(38, "kani_apply_answer_add_credit");
}

#[test]
fn kani_apply_answer_change_request() {
    a_extra::run_event(39, "kani_apply_answer_change_request");
}

#[test]
fn kani_apply_answer_stop() {
    a_extra::run_event(40, "kani_apply_answer_stop");
}

#[test]
fn kani_apply_answer_unclear() {
    a_extra::run_event(41, "kani_apply_answer_unclear");
}

#[test]
fn kani_apply_answer_retry() {
    a_extra::run_event(42, "kani_apply_answer_retry");
}

#[test]
fn kani_apply_answer_mark_handled() {
    a_extra::run_event(43, "kani_apply_answer_mark_handled");
}

#[test]
fn kani_apply_new_adoption() {
    a_extra::run_event(56, "kani_apply_new_adoption");
}

