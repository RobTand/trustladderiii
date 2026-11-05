//! Recovery shell decisions over facts parsed at the adapter boundary.
//! No I/O or clock samples live here. Optional facts preserve short circuits.
pub mod budget {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Legacy {
        Unused,
        Sending,
        Refused,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Stage {
        Ready,
        Sending,
        Spent,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Binding {
        pub repository: bool,
        pub pull_request: bool,
        pub head: bool,
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Legacy receipts consume the head budget.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == match legacy { Legacy::Sending => Some(sending), Legacy::Refused => Some(refused), Legacy::Unused => None }))]
    pub fn legacy_command<T>(legacy: Legacy, sending: T, refused: T) -> Option<T> {
        match legacy {
            Legacy::Sending => Some(sending),
            Legacy::Refused => Some(refused),
            Legacy::Unused => None,
        }
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Never overwrite a spent recovery.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == matches!(stage, Stage::Ready)))]
    pub fn inspect_history(stage: Stage) -> bool {
        matches!(stage, Stage::Ready)
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Retired nodes have no PR/head binding.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (has_pr && has_head)))]
    pub fn legacy_binding(has_pr: bool, has_head: bool) -> bool {
        has_pr && has_head
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Archive history belongs to one repo/PR/head.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (binding.repository && binding.pull_request && binding.head && enqueue)))]
    pub fn archive_candidate(binding: Binding, enqueue: bool) -> bool {
        binding.repository && binding.pull_request && binding.head && enqueue
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Head key precedes effect keys, in archive order.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (if position == 0 { None } else { Some((position - 1) as usize) })))]
    pub fn effect_key_index(position: usize) -> Option<usize> {
        if position == 0 {
            None
        } else {
            Some(position - 1)
        }
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Sent requires this bot, command first line and exact marker line.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (bot && queue_first_line && marker_line)))]
    pub fn sent(bot: bool, queue_first_line: bool, marker_line: bool) -> bool {
        bot && queue_first_line && marker_line
    }
    #[cfg(verus_keep_ghost)]
    vstd::prelude::verus! {
    /// Witnesses for both decisions and boundary values; no bounded search.
    fn witnesses() {

        let observed_0 = legacy_command(Legacy::Sending, 1u8, 2u8);
    assert(observed_0 == Some(1u8));
        let observed_1 = legacy_command(Legacy::Refused, 1u8, 2u8);
    assert(observed_1 == Some(2u8));
        let observed_2 = legacy_command(Legacy::Unused, 1u8, 2u8);
    assert(observed_2.is_none());
        let observed_3 = inspect_history(Stage::Ready);
    assert(observed_3);
        let observed_4 = inspect_history(Stage::Sending);
    assert(!observed_4);
        let observed_5 = inspect_history(Stage::Spent);
    assert(!observed_5);
        let observed_6 = legacy_binding(true, true);
    assert(observed_6);
        let observed_7 = legacy_binding(false, true);
    assert(!observed_7);
        let observed_8 = archive_candidate(Binding {repository: true, pull_request: true, head: true}, true);
    assert(observed_8);
        let observed_9 = archive_candidate(Binding {repository: true, pull_request: true, head: false}, true);
    assert(!observed_9);
        let observed_10 = effect_key_index(0);
    assert(observed_10.is_none());
        let observed_11 = effect_key_index(1);
    assert(observed_11 == Some(0usize));
        let observed_12 = effect_key_index(usize::MAX);
    assert(observed_12 == Some((usize::MAX - 1) as usize));
        let observed_13 = sent(true, true, true);
    assert(observed_13);
        let observed_14 = sent(false, true, true);
    assert(!observed_14);
        let observed_15 = sent(true, false, true);
    assert(!observed_15);
        let observed_16 = sent(true, true, false);
    assert(!observed_16);

    }
    }
}
pub mod eligibility {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Repaired {
        pub phase: bool,
        pub paused: bool,
        pub incident: bool,
        pub open: bool,
        pub head_approved: bool,
        pub checked_approved: bool,
        pub approved_head: Option<bool>,
        pub repository: Option<bool>,
        pub pull_request: Option<bool>,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RepairStep {
        Stop,
        ApprovedHead,
        Repository,
        PullRequest,
        History,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PriorFailure {
        QueueCi,
        Conflict,
        Other,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Eligibility {
        pub current_head: bool,
        pub open: bool,
        pub merged: bool,
        pub draft: bool,
        pub mergeable: bool,
        pub repaired: Option<bool>,
        pub waiting: bool,
        pub settled: bool,
        pub eligible_check: bool,
        pub queue_label: Option<bool>,
        pub held: Option<bool>,
        pub checks_ready: Option<bool>,
        pub reruns_pending: Option<bool>,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum EligibilityStep {
        Stop,
        Repaired,
        Issue,
        Events,
        Checks,
        Reruns,
        Eligible,
    }
    /// docs/state-table.md:537-546, :573-580 Publishing/Queued, Q41/Q47; #447/#608. Preserve snapshot parse gates.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (if !r.phase || r.paused || r.incident || !r.open || !r.head_approved || !r.checked_approved { RepairStep::Stop }
 else { match r.approved_head { None => RepairStep::ApprovedHead, Some(false) => RepairStep::Stop, Some(true) => match r.repository {
 None => RepairStep::Repository, Some(false) => RepairStep::Stop, Some(true) => match r.pull_request {
 None => RepairStep::PullRequest, Some(false) => RepairStep::Stop, Some(true) => RepairStep::History } } } })))]
    pub fn repaired_head(r: Repaired) -> RepairStep {
        if !r.phase || r.paused || r.incident || !r.open || !r.head_approved || !r.checked_approved
        {
            return RepairStep::Stop;
        }
        match r.approved_head {
            None => RepairStep::ApprovedHead,
            Some(false) => RepairStep::Stop,
            Some(true) => match r.repository {
                None => RepairStep::Repository,
                Some(false) => RepairStep::Stop,
                Some(true) => match r.pull_request {
                    None => RepairStep::PullRequest,
                    Some(false) => RepairStep::Stop,
                    Some(true) => RepairStep::History,
                },
            },
        }
    }
    /// docs/state-table.md:573-580 Queued CI/conflict rows, Q47; #447/#608. Only prior queue failure on another head establishes repair.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (repository && pull_request && !matches!(failure, PriorFailure::Other) && other_head)))]
    pub fn prior_failure(
        repository: bool,
        pull_request: bool,
        failure: PriorFailure,
        other_head: bool,
    ) -> bool {
        repository
            && pull_request
            && matches!(failure, PriorFailure::QueueCi | PriorFailure::Conflict)
            && other_head
    }
    /// docs/state-table.md:537-546, :573-580 Publishing/Queued, Q41/Q47; #447/#608. Request later reads only after every earlier eligibility gate.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (if !r.current_head || !r.open || r.merged || r.draft || !r.mergeable { EligibilityStep::Stop }
 else { match r.repaired { None => EligibilityStep::Repaired, Some(false) => EligibilityStep::Stop, Some(true) =>
 if !r.waiting || !r.settled || !r.eligible_check { EligibilityStep::Stop } else { match r.queue_label {
 None => EligibilityStep::Issue, Some(false) => EligibilityStep::Stop, Some(true) => match r.held {
 None => EligibilityStep::Events, Some(true) => EligibilityStep::Stop, Some(false) => match r.checks_ready {
 None => EligibilityStep::Checks, Some(false) => EligibilityStep::Stop, Some(true) => match r.reruns_pending {
 None => EligibilityStep::Reruns, Some(true) => EligibilityStep::Stop, Some(false) => EligibilityStep::Eligible } } } } } } })))]
    pub fn recovery_eligible(r: Eligibility) -> EligibilityStep {
        if !r.current_head || !r.open || r.merged || r.draft || !r.mergeable {
            return EligibilityStep::Stop;
        }
        match r.repaired {
            None => EligibilityStep::Repaired,
            Some(false) => EligibilityStep::Stop,
            Some(true) => {
                if !r.waiting || !r.settled || !r.eligible_check {
                    EligibilityStep::Stop
                } else {
                    match r.queue_label {
                        None => EligibilityStep::Issue,
                        Some(false) => EligibilityStep::Stop,
                        Some(true) => match r.held {
                            None => EligibilityStep::Events,
                            Some(true) => EligibilityStep::Stop,
                            Some(false) => match r.checks_ready {
                                None => EligibilityStep::Checks,
                                Some(false) => EligibilityStep::Stop,
                                Some(true) => match r.reruns_pending {
                                    None => EligibilityStep::Reruns,
                                    Some(true) => EligibilityStep::Stop,
                                    Some(false) => EligibilityStep::Eligible,
                                },
                            },
                        },
                    }
                }
            }
        }
    }
    /// docs/state-table.md:573-580 Queued, Q41 human approval; #608. Without a snapshot there is no proof of explicit resume.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == match unresolved { None => true, Some(value) => value }))]
    pub fn event_hold(unresolved: Option<bool>) -> bool {
        unresolved.unwrap_or(true)
    }
    #[cfg(verus_keep_ghost)]
    vstd::prelude::verus! {
    /// Witnesses for both decisions and boundary values; no bounded search.
    fn witnesses() {

        let r = Repaired {phase: true, paused: false, incident: false, open: true, head_approved: true, checked_approved: true,
            approved_head: None, repository: None, pull_request: None};
        let observed_17 = repaired_head(r);
    assert(observed_17 == RepairStep::ApprovedHead);
        let observed_18 = repaired_head(Repaired {approved_head: Some(true), ..r});
    assert(observed_18 == RepairStep::Repository);
        let observed_19 = repaired_head(Repaired {approved_head: Some(true), repository: Some(true), ..r});
    assert(observed_19 == RepairStep::PullRequest);
        let observed_20 = repaired_head(Repaired {approved_head: Some(true), repository: Some(true), pull_request: Some(true), ..r});
    assert(observed_20 == RepairStep::History);
        let observed_21 = repaired_head(Repaired {paused: true, ..r});
    assert(observed_21 == RepairStep::Stop);
        let observed_22 = prior_failure(true, true, PriorFailure::QueueCi, true);
    assert(observed_22);
        let observed_23 = prior_failure(true, true, PriorFailure::Conflict, true);
    assert(observed_23);
        let observed_24 = prior_failure(true, true, PriorFailure::Other, true);
    assert(!observed_24);
        let observed_25 = prior_failure(true, true, PriorFailure::Conflict, false);
    assert(!observed_25);
        let e = Eligibility {current_head: true, open: true, merged: false, draft: false, mergeable: true,
            repaired: None, waiting: true, settled: true, eligible_check: true, queue_label: None, held: None, checks_ready: None, reruns_pending: None};
        let observed_26 = recovery_eligible(e);
    assert(observed_26 == EligibilityStep::Repaired);
        let observed_27 = recovery_eligible(Eligibility {repaired: Some(true), ..e});
    assert(observed_27 == EligibilityStep::Issue);
        let observed_28 = recovery_eligible(Eligibility {repaired: Some(true), queue_label: Some(true), ..e});
    assert(observed_28 == EligibilityStep::Events);
        let observed_29 = recovery_eligible(Eligibility {repaired: Some(true), queue_label: Some(true), held: Some(false), ..e});
    assert(observed_29 == EligibilityStep::Checks);
        let observed_30 = recovery_eligible(Eligibility {repaired: Some(true), queue_label: Some(true), held: Some(false), checks_ready: Some(true), ..e});
    assert(observed_30 == EligibilityStep::Reruns);
        let observed_31 = recovery_eligible(Eligibility {repaired: Some(true), queue_label: Some(true), held: Some(false), checks_ready: Some(true), reruns_pending: Some(false), ..e});
    assert(observed_31 == EligibilityStep::Eligible);
        let observed_32 = recovery_eligible(Eligibility {draft: true, ..e});
    assert(observed_32 == EligibilityStep::Stop);
        let observed_33 = event_hold(None);
    assert(observed_33); let observed_34 = event_hold(Some(true));
    assert(observed_34); let observed_35 = event_hold(Some(false));
    assert(!observed_35);

    }
    }
}
pub mod schedule {
    use super::budget::Stage;
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RecoveryGate {
        Repaired,
        Pending,
        Proceed,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum DispatchGate {
        Eligibility,
        Holds,
        Ineligible,
    }
    /// docs/state-table.md:573-580 Queued, Q41/Q47; #608. Existing recovery skips repair parsing.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (if saved { RecoveryGate::Proceed } else { match repaired { None => RecoveryGate::Repaired, Some(true) => RecoveryGate::Proceed, Some(false) => RecoveryGate::Pending } })))]
    pub fn recovery_gate(saved: bool, repaired: Option<bool>) -> RecoveryGate {
        if saved {
            RecoveryGate::Proceed
        } else {
            match repaired {
                None => RecoveryGate::Repaired,
                Some(true) => RecoveryGate::Proceed,
                Some(false) => RecoveryGate::Pending,
            }
        }
    }
    /// docs/state-table.md:573-580 Queued, Q41/Q47; #608. An ineligible tick touches an existing recovery only.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == existing))]
    pub fn ineligible_tick(existing: bool) -> bool {
        existing
    }
    /// docs/state-table.md:573-580 Queued, Q41/Q47; #608/#760. Timer reread selects budget adoption without resampling the clock.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == timer.is_some()))]
    pub fn saved_timer(timer: Option<u64>) -> bool {
        timer.is_some()
    }
    /// docs/state-table.md:573-580 Queued, Q41/Q47; #608. Eligibility is sampled only for repaired-head dispatch; holds only when eligible.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (if !waiting { DispatchGate::Holds } else { match eligible { None => DispatchGate::Eligibility, Some(true) => DispatchGate::Holds, Some(false) => DispatchGate::Ineligible } })))]
    pub fn dispatch_gate(waiting: bool, eligible: Option<bool>) -> DispatchGate {
        if !waiting {
            DispatchGate::Holds
        } else {
            match eligible {
                None => DispatchGate::Eligibility,
                Some(true) => DispatchGate::Holds,
                Some(false) => DispatchGate::Ineligible,
            }
        }
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608/#751. Fence only the Sending save.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == matches!(stage, Stage::Sending)))]
    pub fn fence(stage: Stage) -> bool {
        matches!(stage, Stage::Sending)
    }
    #[cfg(verus_keep_ghost)]
    vstd::prelude::verus! {
    /// Witnesses for both decisions and boundary values; no bounded search.
    fn witnesses() {

        let observed_36 = recovery_gate(true, None);
    assert(observed_36 == RecoveryGate::Proceed);
        let observed_37 = recovery_gate(false, None);
    assert(observed_37 == RecoveryGate::Repaired);
        let observed_38 = recovery_gate(false, Some(false));
    assert(observed_38 == RecoveryGate::Pending);
        let observed_39 = recovery_gate(false, Some(true));
    assert(observed_39 == RecoveryGate::Proceed);
        let observed_40 = ineligible_tick(true);
    assert(observed_40); let observed_41 = ineligible_tick(false);
    assert(!observed_41);
        let observed_42 = saved_timer(Some(0));
    assert(observed_42); let observed_43 = saved_timer(Some(u64::MAX));
    assert(observed_43); let observed_44 = saved_timer(None);
    assert(!observed_44);
        let observed_45 = dispatch_gate(false, None);
    assert(observed_45 == DispatchGate::Holds);
        let observed_46 = dispatch_gate(true, None);
    assert(observed_46 == DispatchGate::Eligibility);
        let observed_47 = dispatch_gate(true, Some(false));
    assert(observed_47 == DispatchGate::Ineligible);
        let observed_48 = dispatch_gate(true, Some(true));
    assert(observed_48 == DispatchGate::Holds);
        let observed_49 = fence(Stage::Sending);
    assert(observed_49); let observed_50 = fence(Stage::Ready);
    assert(!observed_50); let observed_51 = fence(Stage::Spent);
    assert(!observed_51);

    }
    }
}
pub mod command {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Reply {
        Accepted,
        Http(u16),
        RateLimited,
        Other,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Outcome {
        Accepted,
        PermissionDenied,
        Rejected,
        Unknown,
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608/#2705. Classification is total, with 403 preceding all other 4xx.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == match reply { Reply::Accepted => Outcome::Accepted, Reply::Http(status) => if status == 403 { Outcome::PermissionDenied } else if 400 <= status && status <= 499 { Outcome::Rejected } else { Outcome::Unknown }, Reply::RateLimited => Outcome::Rejected, Reply::Other => Outcome::Unknown }))]
    pub fn command_result(reply: Reply) -> Outcome {
        match reply {
            Reply::Accepted => Outcome::Accepted,
            Reply::Http(403) => Outcome::PermissionDenied,
            Reply::Http(400..=499) | Reply::RateLimited => Outcome::Rejected,
            Reply::Http(_) | Reply::Other => Outcome::Unknown,
        }
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608. Refusal prevents the potentially destructive comment reconciliation.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (!refusal && comment)))]
    pub fn receipt_found(refusal: bool, comment: bool) -> bool {
        !refusal && comment
    }
    /// docs/state-table.md:537-546 Publishing enqueue, Q41; #608/#2705. Only permission denial returns a permanent completion before propagating errors.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == matches!(outcome, Outcome::PermissionDenied)))]
    pub fn permission_refusal(outcome: Outcome) -> bool {
        matches!(outcome, Outcome::PermissionDenied)
    }
    #[cfg(verus_keep_ghost)]
    vstd::prelude::verus! {
    /// Witnesses for both decisions and boundary values; no bounded search.
    fn witnesses() {

        let observed_52 = command_result(Reply::Accepted);
    assert(observed_52 == Outcome::Accepted);
        let observed_53 = command_result(Reply::Http(403));
    assert(observed_53 == Outcome::PermissionDenied);
        let observed_54 = command_result(Reply::Http(400));
    assert(observed_54 == Outcome::Rejected);
        let observed_55 = command_result(Reply::Http(499));
    assert(observed_55 == Outcome::Rejected);
        let observed_56 = command_result(Reply::RateLimited);
    assert(observed_56 == Outcome::Rejected);
        let observed_57 = command_result(Reply::Http(399));
    assert(observed_57 == Outcome::Unknown);
        let observed_58 = command_result(Reply::Http(500));
    assert(observed_58 == Outcome::Unknown);
        let observed_59 = command_result(Reply::Http(u16::MAX));
    assert(observed_59 == Outcome::Unknown);
        let observed_60 = command_result(Reply::Other);
    assert(observed_60 == Outcome::Unknown);
        let observed_61 = receipt_found(false, true);
    assert(observed_61); let observed_62 = receipt_found(true, true);
    assert(!observed_62); let observed_63 = receipt_found(false, false);
    assert(!observed_63);
        let observed_64 = permission_refusal(Outcome::PermissionDenied);
    assert(observed_64); let observed_65 = permission_refusal(Outcome::Unknown);
    assert(!observed_65);

    }
    }
}
