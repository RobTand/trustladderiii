//! G21 (K5 sweep NEW-4): every event variant and every phase names the Verus
//! groups that carry its obligations and at least one cover that reaches it.
//!
//! `obligations_of_event` and `obligations_of_phase` are matches with no
//! wildcard arm over Event, Reply, Ack, Fact, Command, Answer and Phase. A new
//! variant does not compile until it gets an entry. This replaces the
//! exhaustiveness that lib.rs `proof_of_event` / `proofs_of_phase` give the
//! Kani map, which K5 deletes. The tests check that each named group is gated
//! in tools/bazel/verus.bzl and that each named cover exists.
//!
//! First cut. The group lists align with Apply's relation after G9 and the
//! wildcard-free rule functions (G21 layer 2, Apply's lease).

use std::collections::BTreeSet;
use tl3_kernel::*;

const VERUS_GROUPS: &str = include_str!("../../../tools/bazel/verus.bzl");
const TRANSITION_COVERS: &str = include_str!("transition_covers.rs");
const FAIL_FAST: &str = include_str!("fail_fast.rs");
const SCENARIOS: &str = include_str!("scenarios.rs");
const STEP_REPORT: &str = include_str!("step_report.rs");
const DEPENDENCY_SAFETY: &str = include_str!("dependency_safety.rs");
const THIS: &str = include_str!("obligation_map.rs");

#[derive(Clone, Copy, Debug)]
enum File {
    TransitionCovers,
    FailFast,
    Scenarios,
    StepReport,
    DependencySafety,
}

impl File {
    fn source(self) -> &'static str {
        match self {
            File::TransitionCovers => TRANSITION_COVERS,
            File::FailFast => FAIL_FAST,
            File::Scenarios => SCENARIOS,
            File::StepReport => STEP_REPORT,
            File::DependencySafety => DEPENDENCY_SAFETY,
        }
    }
}

/// A witness that the variant or phase is reachable and changes something.
#[derive(Clone, Copy, Debug)]
enum Cover {
    /// A quoted row label in transition_covers.rs: an
    /// `every_event_family_can_change_the_world` family or an
    /// `every_phase_moves_under_settle_or_waits` phase.
    Row(&'static str),
    /// A `#[test]` function.
    Test(File, &'static str),
    /// A gated Verus witness group.
    Witness(&'static str),
}

use Cover::{Row, Test, Witness};
use File::{DependencySafety, FailFast, Scenarios, StepReport, TransitionCovers};

struct Obligations {
    groups: &'static [&'static str],
    covers: &'static [Cover],
}

const fn ob(groups: &'static [&'static str], covers: &'static [Cover]) -> Obligations {
    Obligations { groups, covers }
}

/// Groups every event passes through: dispatch and the Apply relation.
const EVENT_GROUPS: &[&str] = &["events", "transition_safety_apply_rules", "transition_safety_compose"];
/// Groups every reply passes through: admission.
const REPLY_GROUPS: &[&str] = &["reply_admission", "reply_admission_on_reply"];
/// Groups every phase's internal step passes through.
const PHASE_GROUPS: &[&str] = &["internal_safety", "ranks_advance_node", "progress_global"];

fn obligations_of_event(event: &Event) -> Obligations {
    match event {
        Event::NewRequest { .. } => ob(&["events"], &[Row("apply_new_request")]),
        Event::NewAdoption { .. } => ob(&["events"], &[Row("apply_new_adoption")]),
        Event::Reply { reply, .. } => obligations_of_reply(reply),
        Event::Ack { ack, .. } => obligations_of_ack(*ack),
        Event::Fact { fact, .. } => obligations_of_fact(fact),
        Event::Command { command, .. } => obligations_of_command(command),
    }
}

fn obligations_of_reply(reply: &Reply) -> Obligations {
    match reply {
        Reply::IngestDone { .. } => ob(&["ingest_reply_ingest", "ingest_finish_ingest"], &[Row("apply_ingest_done")]),
        Reply::IngestEarlyEnd(_) => ob(&["ingest_reply_ingest"], &[Row("apply_ingest_early_end")]),
        Reply::AuditAgrees => ob(&["ingest_reply_ingest"], &[Row("apply_audit_agrees")]),
        Reply::AuditDisagrees => ob(
            &["ingest_reply_ingest", "ingest_audit_disagreement_node"],
            &[
                Test(FailFast, "second_audit_disagreement_never_asks_or_spends"),
                Test(Scenarios, "an_audit_disagreement_is_revised_once_before_stopping_for_review"),
            ],
        ),
        Reply::PlanLeaf { .. } => ob(&["work_replies_plan"], &[Row("apply_plan_leaf")]),
        Reply::PlanSplit => ob(&["work_replies_plan"], &[Row("apply_plan_split")]),
        Reply::PlanCantMeet => ob(&["work_replies_plan", "t860_retries_plan_retry"], &[Row("apply_plan_cant_meet")]),
        Reply::PlanCheckRejected => ob(&["plan_check"], &[Witness("plan_check_witness")]),
        Reply::PlanCheckNeedsSplit => ob(&["plan_check"], &[Witness("plan_check_witness")]),
        Reply::ProtectedPaths => ob(
            &["ingest_reply_ingest"],
            &[
                Row("apply_protected_paths"),
                Test(Scenarios, "protected_paths_close_from_ingest_audit_or_planning_without_credit_or_questions"),
            ],
        ),
        Reply::MakerDone { .. } => ob(&["work_replies"], &[Row("apply_maker_done")]),
        Reply::MakerNeedsFiles { .. } => ob(&["reply_admission_files"], &[Row("apply_maker_needs_files")]),
        Reply::MakerCantMeet => ob(&["work_replies"], &[Row("apply_maker_cant_meet")]),
        Reply::MakerOutOfRoom => ob(&["work_replies", "t860_retries_out_of_room"], &[Row("apply_maker_out_of_room")]),
        Reply::ChecksDone { .. } => ob(&["work_replies", "t860_retries_failed_round"], &[Row("apply_checks_done")]),
        Reply::Approved { .. } => ob(&["work_replies", "work_replies_recheck_head"], &[Row("apply_approved")]),
        Reply::ChangesRequested => ob(&["work_replies", "t860_retries_failed_round"], &[Row("apply_changes_requested")]),
        Reply::ReviewerOutOfRoom => ob(
            &["work_replies", "t860_retries_out_of_room"],
            &[Row("apply_reviewer_out_of_room")],
        ),
        Reply::RebaseClean { .. } => ob(&["work_replies_rebase"], &[Row("apply_rebase_clean")]),
        Reply::RebaseNeedsChanges => ob(&["work_replies_rebase"], &[Row("apply_rebase_needs_changes")]),
        Reply::RunFailed => ob(&["retries"], &[Row("apply_run_failed")]),
    }
}

fn obligations_of_ack(ack: Ack) -> Obligations {
    match ack {
        Ack::Confirmed => ob(
            &["acknowledgements_on_ack", "acknowledgements_confirmed", "acknowledgements_confirmed_agenda"],
            &[Row("apply_ack_confirmed")],
        ),
        Ack::DeliveryAccepted => ob(
            &["acknowledgements_on_ack", "acknowledgements_delivery_accepted"],
            &[Row("apply_ack_delivery_accepted")],
        ),
        Ack::Retryable => ob(
            &["acknowledgements_on_ack", "acknowledgements_delivery_failed"],
            &[Row("apply_ack_retryable")],
        ),
        Ack::Permanent => ob(
            &["acknowledgements_on_ack", "acknowledgements_effect_failed"],
            &[Row("apply_ack_permanent")],
        ),
        Ack::BodyPolicyRefused => ob(
            &["acknowledgements_on_ack", "acknowledgements_effect_failed"],
            &[Row("apply_ack_body_policy_refused")],
        ),
    }
}

fn obligations_of_fact(fact: &Fact) -> Obligations {
    match fact {
        Fact::DependencyRemove { .. } => ob(
            &["facts", "dependencies_remove_preserves"],
            &[
                Witness("dependency_preservation_removal_covers"),
                Test(DependencySafety, "removal_is_a_stable_filter_only_while_ingesting"),
            ],
        ),
        Fact::Dependency { .. } => ob(
            &["facts", "dependencies_fact_preserves", "dependency_retirement_fact"],
            &[
                Witness("dependency_retirement_fact_witness"),
                Test(DependencySafety, "registration_appends_only_before_planning"),
            ],
        ),
        Fact::PrMerged { .. } => ob(
            &["facts", "facts_merge", "retirement_verify_selected_apply"],
            &[
                Witness("retirement_merge_witness"),
                Test(Scenarios, "a_clean_request_merges_without_asking_rob"),
            ],
        ),
        Fact::PrClosedByPerson => ob(
            &["facts", "facts_transition"],
            &[Test(StepReport, "pr_closed_is_distinct_from_a_user_stop")],
        ),
        Fact::PushByOther { .. } => ob(&["facts", "facts_transition"], &[Row("apply_outside_push")]),
        Fact::CiFailed { .. } => ob(
            &["facts", "facts_ci_failed", "facts_ci_infrastructure"],
            &[Row("apply_ci_failed"), Test(TransitionCovers, "ci_failure_in_round_one_under_fail_fast_stops_for_review")],
        ),
        Fact::Conflict => ob(&["facts", "facts_conflict", "apply_safety_conflict_rule"], &[Row("advance_rebasing")]),
        Fact::IncidentCleared { .. } => ob(
            &["facts", "facts_incident_cleared"],
            &[Test(Scenarios, "a_crash_loop_reaches_rob_after_its_second_incident")],
        ),
        Fact::Priority(_) => ob(
            &["facts", "facts_transition", "facts_transition_priority"],
            &[Test(Scenarios, "a_priority_fact_refreshes_an_open_request_and_leaves_a_closed_one")],
        ),
    }
}

fn obligations_of_command(command: &Command) -> Obligations {
    match command {
        Command::Stop => ob(&["commands", "handler_safety_stop_step"], &[Row("apply_stop")]),
        Command::Pause => ob(&["commands"], &[Row("apply_pause")]),
        Command::Resume => ob(&["commands"], &[Row("apply_resume")]),
        Command::ChangeRequest => ob(&["commands", "handler_safety_stop_step"], &[Row("apply_change_request")]),
        Command::Answer { answer, .. } => obligations_of_answer(*answer),
    }
}

fn obligations_of_answer(answer: Answer) -> Obligations {
    match answer {
        Answer::Continue => ob(&["answers", "command_helpers_allowed", "continuations"], &[Row("apply_answer_continue")]),
        Answer::TryAgain => ob(&["answers", "command_helpers_allowed"], &[Row("apply_answer_try_again")]),
        Answer::AddCredit(_) => ob(&["answers", "command_helpers_allowed"], &[Row("apply_answer_add_credit")]),
        Answer::ChangeRequest => ob(&["answers", "command_helpers_allowed"], &[Row("apply_answer_change_request")]),
        Answer::Stop => ob(&["answers", "command_helpers_allowed"], &[Row("apply_answer_stop")]),
        Answer::Unclear => ob(&["answers"], &[Row("apply_answer_unclear")]),
        Answer::Retry => ob(&["answers", "command_helpers_allowed"], &[Row("apply_answer_retry")]),
        Answer::MarkHandled => ob(
            &["answers", "command_helpers_allowed", "command_helpers_mark_handled"],
            &[Row("apply_answer_mark_handled")],
        ),
    }
}

fn obligations_of_phase(phase: Phase) -> Obligations {
    match phase {
        Phase::Ingesting => ob(&[], &[Row("advance_ingesting")]),
        Phase::Auditing => ob(&[], &[Row("advance_auditing")]),
        Phase::Unplanned => ob(
            &["ranks_advance_admit", "special_safety", "progress_unplanned"],
            &[Row("advance_unplanned"), Test(TransitionCovers, "w_slot_waiters_block_planning_and_fewer_admit_it")],
        ),
        Phase::Planning { .. } => ob(&["progress_gate"], &[Row("advance_planning")]),
        Phase::Ready(_) => ob(
            &["ranks_advance_slot", "ranks_advance_start", "special_safety", "progress_gate", "progress_pool"],
            &[
                Row("advance_ready"),
                Test(TransitionCovers, "gate_grants_the_last_available_slot"),
                Test(TransitionCovers, "the_pool_goes_to_the_better_priority_and_the_older_ticket_waits"),
            ],
        ),
        Phase::Making => ob(&["progress_gate_maker"], &[Row("advance_making")]),
        Phase::Checking => ob(&["ranks_advance_checks"], &[Row("advance_checking")]),
        Phase::Reviewing => ob(&[], &[Row("advance_reviewing")]),
        Phase::Publishing => ob(&[], &[Row("advance_publishing")]),
        // Waits: settle leaves a delivered request unchanged.
        Phase::AwaitingDelivery => ob(
            &["progress_phase_late_cover"],
            &[Test(TransitionCovers, "every_phase_moves_under_settle_or_waits")],
        ),
        Phase::Rebasing => ob(&["ranks_advance_rebase", "special_safety"], &[Row("advance_rebasing")]),
        Phase::AwaitingRob => ob(
            &[],
            &[Test(TransitionCovers, "reviewer_out_of_room_first_exhaustion_needs_credit")],
        ),
        Phase::Stopping(_) => ob(
            &["ranks_advance_teardown", "allocation_stopping_progress"],
            &[Row("advance_stopping")],
        ),
        Phase::Closed(_) => ob(&[], &[Test(Scenarios, "cleanup_that_fails_waits_for_rob_then_closes")]),
    }
}

/// One value of every variant. The matches above are exhaustive at compile
/// time; `the_samples_visit_every_mapped_variant` keeps this list in step.
fn every_event() -> Vec<Event> {
    let footprint = World::new(&[1], 1).intern_footprint([FileId(0)]);
    let node = NodeId(0);
    let reply = |reply| Event::Reply { node, generation: 0, reply };
    let ack = |ack| Event::Ack { node, seq: 0, ack };
    let fact = |fact| Event::Fact { node, fact };
    let command = |command| Event::Command { node, id: 0, command };
    let answer = |answer| command(Command::Answer { question: 0, answer });
    let head = HeadId(1);
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 1, request: None };
    vec![
        Event::NewRequest { repo: RepoId(0), priority: Priority::Unranked },
        Event::NewAdoption { repo: RepoId(0), head, priority: Priority::Unranked },
        reply(Reply::IngestDone { size: 1 }),
        reply(Reply::IngestEarlyEnd(EarlyEnd::Duplicate)),
        reply(Reply::AuditAgrees),
        reply(Reply::AuditDisagrees),
        reply(Reply::PlanLeaf { footprint, fits: true }),
        reply(Reply::PlanSplit),
        reply(Reply::PlanCantMeet),
        reply(Reply::PlanCheckRejected),
        reply(Reply::PlanCheckNeedsSplit),
        reply(Reply::ProtectedPaths),
        reply(Reply::MakerDone { head }),
        reply(Reply::MakerNeedsFiles { files: footprint, fits: true }),
        reply(Reply::MakerCantMeet),
        reply(Reply::MakerOutOfRoom),
        reply(Reply::ChecksDone { report: true, pass: true, in_footprint: true }),
        reply(Reply::Approved { head }),
        reply(Reply::ChangesRequested),
        reply(Reply::ReviewerOutOfRoom),
        reply(Reply::RebaseClean { head }),
        reply(Reply::RebaseNeedsChanges),
        reply(Reply::RunFailed),
        ack(Ack::Confirmed),
        ack(Ack::DeliveryAccepted),
        ack(Ack::Retryable),
        ack(Ack::Permanent),
        ack(Ack::BodyPolicyRefused),
        fact(Fact::DependencyRemove { kind: PrerequisiteKind::Issue, number: 1 }),
        fact(Fact::Dependency { target, status: DependencyStatus::Pending }),
        fact(Fact::PrMerged { head, checks_passed: true }),
        fact(Fact::PrClosedByPerson),
        fact(Fact::PushByOther { head }),
        fact(Fact::CiFailed { infra: false }),
        fact(Fact::Conflict),
        fact(Fact::IncidentCleared { by_rob: false }),
        fact(Fact::Priority(Priority::Unranked)),
        command(Command::Stop),
        command(Command::Pause),
        command(Command::Resume),
        command(Command::ChangeRequest),
        answer(Answer::Continue),
        answer(Answer::TryAgain),
        answer(Answer::AddCredit(1)),
        answer(Answer::ChangeRequest),
        answer(Answer::Stop),
        answer(Answer::Unclear),
        answer(Answer::Retry),
        answer(Answer::MarkHandled),
    ]
}

fn every_phase() -> Vec<Phase> {
    vec![
        Phase::Ingesting,
        Phase::Auditing,
        Phase::Unplanned,
        Phase::Planning { must_split: false },
        Phase::Ready(Resume::Making),
        Phase::Making,
        Phase::Checking,
        Phase::Reviewing,
        Phase::Publishing,
        Phase::AwaitingDelivery,
        Phase::Rebasing,
        Phase::AwaitingRob,
        Phase::Stopping(Outcome::Stopped),
        Phase::Closed(Outcome::Stopped),
    ]
}

const ENUMS: &[&str] = &["Event", "Reply", "Ack", "Fact", "Command", "Answer", "Phase"];

/// `Enum::Variant` paths of the seven mapped enums in `src`.
fn variant_paths(src: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for e in ENUMS {
        let prefix = format!("{e}::");
        for (at, _) in src.match_indices(&prefix) {
            if src[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = &src[at + prefix.len()..];
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
            paths.insert(format!("{prefix}{name}"));
        }
    }
    paths
}

fn section(start: &str, end: &str) -> &'static str {
    let from = THIS.find(start).expect(start);
    let to = from + THIS[from..].find(end).expect(end);
    &THIS[from..to]
}

fn gated(group: &str) -> bool {
    VERUS_GROUPS.contains(&format!("\"{group}\": ("))
}

fn cover_exists(cover: Cover) -> bool {
    match cover {
        Row(label) => TRANSITION_COVERS.contains(&format!("\"{label}\"")),
        Test(file, name) => file.source().contains(&format!("#[test]\nfn {name}(")),
        Witness(group) => gated(group),
    }
}

fn check(what: &str, groups: &[&str], ob: &Obligations, missing: &mut Vec<String>) {
    for group in groups.iter().chain(ob.groups) {
        if !gated(group) {
            missing.push(format!("{what}: ungated group {group}"));
        }
    }
    if ob.covers.is_empty() {
        missing.push(format!("{what}: no cover"));
    }
    for &cover in ob.covers {
        if !cover_exists(cover) {
            missing.push(format!("{what}: missing cover {cover:?}"));
        }
    }
}

#[test]
fn every_event_variant_names_gated_groups_and_an_existing_cover() {
    let mut missing = Vec::new();
    for event in &every_event() {
        let mut shared = EVENT_GROUPS.to_vec();
        if let Event::Reply { .. } = event {
            shared.extend(REPLY_GROUPS);
        }
        check(&format!("{event:?}"), &shared, &obligations_of_event(event), &mut missing);
    }
    assert!(missing.is_empty(), "{missing:#?}");
}

#[test]
fn every_phase_names_gated_groups_and_an_existing_cover() {
    let mut missing = Vec::new();
    for phase in every_phase() {
        check(&format!("{phase:?}"), PHASE_GROUPS, &obligations_of_phase(phase), &mut missing);
    }
    assert!(missing.is_empty(), "{missing:#?}");
}

/// The guard only holds while the maps keep their exhaustive matches.
#[test]
fn the_obligation_maps_have_no_wildcard_arm() {
    let maps = section("fn obligations_of_event(", "/// One value of every variant.");
    assert!(!maps.contains("_ =>"), "wildcard arm in the obligation maps");
    assert!(!maps.contains("| _"), "wildcard alternative in the obligation maps");
}

/// A variant mapped but missing from the samples would go unchecked.
#[test]
fn the_samples_visit_every_mapped_variant() {
    let maps = variant_paths(section("fn obligations_of_event(", "/// One value of every variant."));
    let samples = variant_paths(section("fn every_event()", "const ENUMS"));
    // The samples reach these through the reply/ack/fact/command/answer helpers.
    let wrappers = ["Event::Reply", "Event::Ack", "Event::Fact", "Event::Command", "Command::Answer"];
    let unvisited: Vec<_> = maps.iter().filter(|p| !samples.contains(*p) && !wrappers.contains(&p.as_str())).collect();
    assert!(unvisited.is_empty(), "mapped but not sampled: {unvisited:?}");
    assert_eq!(maps.len(), 68, "7 enums: 6 + 21 + 5 + 9 + 5 + 8 + 14 variants (maps: {maps:?})");
}
