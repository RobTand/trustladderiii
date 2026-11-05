//! #806: unchanged kernel helpers, independently gated for bounded solver time.
use super::*;

kernel_fn! {
    pub(super) fn agenda(phase: Phase) -> (result: &'static [Item]) [
        ensures result@ == items(phase), result.len() <= 5,
    ] {
        use EffectKind as K;
        use Item as I;
        match phase {
            Phase::Ingesting => &[I::Start(Role::Ingest), I::Await],
            Phase::Auditing => &[I::Start(Role::Audit), I::Await],
            Phase::Unplanned => &[I::Admit],
            Phase::Planning { .. } => &[I::Start(Role::Plan), I::Await],
            Phase::Ready(_) => &[I::Slot],
            Phase::Making => &[I::Do(K::PrepareWorktree), I::Start(Role::Make), I::Await],
            Phase::Checking => &[I::StartChecks, I::Await],
            Phase::Reviewing => &[I::Start(Role::Review), I::Await],
            Phase::Publishing => &[
                I::Do(K::Push),
                I::Do(K::PostApproval),
                I::Do(K::OpenPr),
                I::Do(K::Enqueue),
            ],
            Phase::Queued => &[I::Merge],
            Phase::Rebasing => &[I::StartRebase, I::Await],
            Phase::AwaitingRob => &[I::Answer],
            Phase::Stopping(_) => &[
                I::Teardown(K::StopRun),
                I::Teardown(K::WithdrawJob),
                I::Teardown(K::ClosePr),
                I::Release,
                I::Teardown(K::RemoveWorktree),
            ],
            Phase::Closed(_) => &[],
        }
    }
}

kernel_fn! {
/// #771: a retried run starts at its original operation agenda entry.
pub(super) fn operation_start(phase: Phase) -> (result: u8) [
    ensures result == if phase == Phase::Making { 1u8 } else { 0u8 },
] {
    match phase {
        Phase::Making => 1,
        _ => 0,
    }
}
}

kernel_fn! {
    pub(super) fn rank(phase: Phase) -> (result: u64) [
        ensures result as int == phase_rank(phase), result <= 32,
        decreases if phase is Ready { 1nat } else { 0nat },
    ] {
        match phase {
            Phase::Closed(_) => 0,
            Phase::Stopping(_) => 2,
            Phase::Queued => 4,
            Phase::Publishing => 6,
            Phase::Reviewing => 10,
            Phase::Checking => 14,
            Phase::Rebasing => 18,
            Phase::Making => 22,
            Phase::Ready(r) => rank(r.phase()) + 2,
            Phase::Planning { .. } => 26,
            Phase::Unplanned => 28,
            Phase::Auditing => 30,
            Phase::Ingesting => 32,
            Phase::AwaitingRob => 3,
        }
    }
}

kernel_fn! {
    pub(super) fn cont_rank(cont: Cont) -> (result: u64) [
        ensures result as int == continuation_rank(cont), result <= 32,
    ] {
        match cont {
            Cont::Ingesting => 32,
            Cont::Auditing => 30,
            Cont::Unplanned => 28,
            Cont::Planning { .. } => 26,
            Cont::Ready(r) => rank(Phase::Ready(r)),
            Cont::Stop(_) | Cont::Nowhere => 2,
        }
    }
}
