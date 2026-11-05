#[path = "../src/looks_state.rs"]
mod looks_state;
fn main() {}
#[cfg(verus_keep_ghost)]
mod witnesses {
    use super::looks_state::*;
    use vstd::prelude::*;
    verus! {
    /// Q53/Q43/Q7, state-table.md:277,792-814 / #759: concrete witnesses for every D2 branch.
    /// Contracts are unbounded over their integer types; this lemma establishes reachability only.
    fn d2_witnesses() {
        let ready=quota_wait(10,10,Some(0),false,5);
        let wait=quota_wait(20,10,Some(8),false,5);
        let note=quota_wait(20,10,Some(5),false,5);
        let noticed=quota_wait(20,10,Some(5),true,5);
        let unknown=quota_wait(20,10,None,false,5);
        let cap=quota_wait(u64::MAX,10,Some(u64::MAX),false,1);
        assert(ready==QuotaWait::Resume && wait==QuotaWait::Wait {due:13});
        assert(note==QuotaWait::Notify {due:20} && noticed==QuotaWait::Wait {due:20});
        assert(unknown==QuotaWait::Wait {due:20} && cap==QuotaWait::Wait {due:u64::MAX});
        let resume=entry_action(true,true); let stop=entry_action(false,true); let look=entry_action(false,false);
        assert(resume==EntryAction::ResumeQuota && stop==EntryAction::FinishStall && look==EntryAction::Look);
        let missing=missing_is_absent(true,true,true); let first=missing_is_absent(true,false,true);
        assert(missing && !first);
        let healthy=read_decision(LookKind::Healthy,true);let waiting=read_decision(LookKind::Waiting,true);
        let pending=read_decision(LookKind::Pending,false); let failed=read_decision(LookKind::Failed,true);
        assert(healthy.reset_failures && healthy.clear_wait && !waiting.reset_failures && !waiting.clear_wait);
        assert(!pending.reset_failures && !pending.clear_wait && !failed.reset_failures && failed.clear_wait);
        let complete=found_action(true,false,false,false); let observe=found_action(false,false,true,true);
        let setup=found_action(false,true,false,false); let skip=found_action(false,false,true,false);
        assert(complete==FoundAction::Complete && observe==FoundAction::Observe && setup==FoundAction::Observe && skip==FoundAction::Skip);
        let release=absence_action(true,true,true,true,false,true);
        let retry=absence_action(false,true,false,true,false,true);
        let cancelling=absence_action(false,true,false,true,true,true);
        assert(release==AbsenceAction::ReleaseHold && retry==AbsenceAction::Retry && cancelling==AbsenceAction::Ignore);
        let timely=alive_resets_early(false,9,0,10); let beyond=alive_resets_early(false,10,0,10);
        let backwards=alive_resets_early(false,0,10,1); let changed=alive_resets_early(true,10,0,0);
        assert(timely && !beyond && backwards && changed);
        let alive=wait_action(WaitStatus::Alive,true); let ended=wait_action(WaitStatus::Ended,false);
        let repeated=wait_action(WaitStatus::Ended,true); let error=wait_action(WaitStatus::Failed,false);
        assert(alive==WaitAction::WatchAlive && ended==WaitAction::WatchEnded {start_clock:true});
        assert(repeated==WaitAction::WatchEnded {start_clock:false} && error==WaitAction::Fail);
        let future=quota_due(20,10);let past=quota_due(5,10);
        assert(future==20 && past==10);
        let progress=progress_due(10,20,5); let saturated=progress_due(u64::MAX,1,1);
        assert(progress==15 && saturated==u64::MAX);
        let initial=record_quota(None,10);let newer=record_quota(Some(10),11);let same=record_quota(Some(10),10);
        assert(initial && newer && !same);
        let replay=quota_replayed(Some(10),true,10);let other=quota_replayed(Some(10),false,10);let fresh=quota_replayed(None,true,10);
        assert(replay && !other && !fresh);
        ladder_note_once(100, 20, 0, 10);
    }

    /// #759 / Q53, state-table.md:277,792: one note per unserved period, then wait only until the announced reset.
    /// Universal over clock/deadline values. Progress resets the shell period in progress_allowance.
    fn ladder_note_once(resume: u64, now: u64, since: u64, delay: u64)
        requires resume > now, since as int + delay as int <= now as int,
    {
        let first = quota_wait(resume, now, Some(since), false, delay);
        assert(first == QuotaWait::Notify { due: resume });
        // The real shell sets ladder_out_noticed before writing the note.
        let next = quota_wait(resume, now, Some(since), true, delay);
        assert(next == QuotaWait::Wait { due: resume });
        let reset = quota_wait(resume, resume, Some(since), true, delay);
        assert(reset == QuotaWait::Resume);
    }
    }
}
