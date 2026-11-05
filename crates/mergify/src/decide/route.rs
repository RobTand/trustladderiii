//! Queue-fact read gates and ingress routing. The shell retains all sampling and I/O.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

pub mod queue {
    #[cfg(verus_keep_ghost)]
    use super::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Setup {
        pub unavailable: bool,
        pub missing_suites: bool,
    }
    /// Q52, docs/state-table.md:542: unavailable permissions and missing contexts have independent setup gates.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out.unavailable == !permissions_empty && out.missing_suites == !missing_empty,))]
    pub fn setup(permissions_empty: bool, missing_empty: bool) -> Setup {
        Setup {
            unavailable: !permissions_empty,
            missing_suites: !missing_empty,
        }
    }
    /// #447, docs/state-table.md:545,578: blocked suites are read only for settled Waiting with unmet conditions.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (waiting && settled && unmet),))]
    pub fn blocked_suites(waiting: bool, settled: bool, unmet: bool) -> bool {
        waiting && settled && unmet
    }
    /// #447, docs/state-table.md:545: only completed Waiting can contribute unmet queue conditions.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (waiting && completed),))]
    pub fn conditions(waiting: bool, completed: bool) -> bool {
        waiting && completed
    }
    /// #447 required-set concatenation preserves existing behaviour (no rule): missing then reported, including duplicates.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out@ == missing@ + reported@,))]
    pub fn required<T>(mut missing: Vec<T>, mut reported: Vec<T>) -> Vec<T> {
        missing.append(&mut reported);
        missing
    }
    /// Q52, docs/state-table.md:543: CheckReported belongs to both the open incidents and reported requirements.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (open && has_report),))]
    pub fn reported(open: bool, has_report: bool) -> bool {
        open && has_report
    }
    /// Preserved existing behaviour (no rule): recorded evidence skips detail reads and emission.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!recorded),))]
    pub fn emit(recorded: bool) -> bool {
        !recorded
    }
    /// Preserved existing behaviour (no rule): only the exact queue merge title counts as queue evidence.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (exact_title),))]
    pub fn merged_title(exact_title: bool) -> bool {
        exact_title
    }

    #[cfg(verus_keep_ghost)]
    verus! {
    /// Q52/#447, docs/state-table.md:542-545: gates and CheckReported have positive and rejecting witnesses.
    fn gates_witness() {
        let setup_ = setup(false, false); assert(setup_.unavailable && setup_.missing_suites);
        let empty = setup(true, true); assert(!empty.unavailable && !empty.missing_suites);
        let blocked = blocked_suites(true, true, true); assert(blocked);
        let running = blocked_suites(true, false, true); assert(!running);
        let other = blocked_suites(false, true, true); assert(!other);
        let met = blocked_suites(true, true, false); assert(!met);
        let conditions_ = conditions(true, true); assert(conditions_);
        let waiting = conditions(true, false); assert(!waiting);
        let report = reported(true, true); assert(report);
        let absent = reported(true, false); assert(!absent);
        let closed = reported(false, true); assert(!closed);
    }
    /// Preserved existing behaviour (no rule): ordered required concatenation, dedupe and title witnesses.
    fn selection_witness() {
        let list = required(vec![1u8, 2u8], vec![2u8, 3u8]); assert(list@ == seq![1u8,2u8,2u8,3u8]);
        let empty = required::<u8>(vec![],vec![]); assert(empty@.len() == 0);
        let fresh = emit(false); assert(fresh);
        let old = emit(true); assert(!old);
        let title = merged_title(true); assert(title);
        let other = merged_title(false); assert(!other);
    }
    }
}

pub mod webhook {
    #[cfg(verus_keep_ghost)]
    use super::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Event {
        Merged,
        Closed,
        Unlabeled,
        Synchronized,
        CheckRun,
        CheckSuite,
        MergeGroup,
        Other,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Check {
        Queue,
        Other,
        Drop,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Action {
        pub look: bool,
        pub wake: bool,
    }
    /// Preserved existing behaviour (no rule): only PR state, check and merge-group events reach queue routing.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!matches!(event, Event::Other)),))]
    pub fn interested(event: Event) -> bool {
        !matches!(event, Event::Other)
    }
    /// Preserved existing behaviour (no rule): installation matches exactly and repo names match ignoring ASCII case.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (installation && owner && name),))]
    pub fn repository(installation: bool, owner: bool, name: bool) -> bool {
        installation && owner && name
    }
    /// Preserved existing behaviour (no rule): only bound, not-yet-merged TL3 PRs yield merged queue facts.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (bound && unmerged && !queue_pr),))]
    pub fn merged(bound: bool, unmerged: bool, queue_pr: bool) -> bool {
        bound && unmerged && !queue_pr
    }
    /// Preserved existing behaviour (no rule): queue PRs and untracked PR updates stop before upstream reads.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!queue_pr && tracked),))]
    pub fn pull(queue_pr: bool, tracked: bool) -> bool {
        !queue_pr && tracked
    }
    /// docs/state-table.md:667 PushByOther: a synchronized head is attributed only to an actor other than TL3; exact actor/head comparison preserves existing behaviour.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!app_bot && same_head),))]
    pub fn push(app_bot: bool, same_head: bool) -> bool {
        !app_bot && same_head
    }
    /// Preserved existing behaviour (no rule): queue-name checks require BOTH exact Mergify app id and slug; spoofed queue checks are dropped.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (if queue_name { if app_id && app_slug { Check::Queue } else { Check::Drop } } else { Check::Other }),))]
    pub fn check(queue_name: bool, app_id: bool, app_slug: bool) -> Check {
        if queue_name {
            if app_id && app_slug {
                Check::Queue
            } else {
                Check::Drop
            }
        } else {
            Check::Other
        }
    }
    /// Preserved existing behaviour (no rule): a check/suite targets a tracked head and either no PR list or one containing its number.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (head && (empty || listed)),))]
    pub fn head_pull(head: bool, empty: bool, listed: bool) -> bool {
        head && (empty || listed)
    }
    /// Q52, docs/state-machine.md:720; queue-event waking preserves existing behaviour (no rule): non-queue checks look and wake only a pending enqueue on its approved head.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out.look == (queue_event || pending && approved) && out.wake == (!queue_event && pending && approved),))]
    pub fn check_action(queue_event: bool, pending: bool, approved: bool) -> Action {
        Action {
            look: queue_event || pending && approved,
            wake: !queue_event && pending && approved,
        }
    }
    /// Q52, docs/state-machine.md:720: the completed suite looks and wakes pending enqueues on the approved head.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out.look == pending && out.wake == pending,))]
    pub fn suite_action(pending: bool) -> Action {
        Action {
            look: pending,
            wake: pending,
        }
    }
    /// Preserved existing behaviour (no rule): a fresh merged PR uses look instead of person events.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (merged),))]
    pub fn fresh_merge(merged: bool) -> bool {
        merged
    }

    #[cfg(verus_keep_ghost)]
    verus! {
    /// Preserved existing behaviour (no rule): event, repository and bound merge gates are reachable in both directions.
    fn ingress_witness() {
        let merged_ = interested(Event::Merged); assert(merged_);
        let other = interested(Event::Other); assert(!other);
        let repo = repository(true,true,true); assert(repo);
        let foreign = repository(false,true,true); assert(!foreign);
        let ours = merged(true,true,false); assert(ours);
        let queue = merged(true,true,true); assert(!queue);
        let unbound = merged(false,true,false); assert(!unbound);
        let done = merged(true,false,false); assert(!done);
        let selected = pull(false,true); assert(selected);
        let internal = pull(true,true); assert(!internal);
        let absent = pull(false,false); assert(!absent);
    }
    /// docs/state-table.md:667 PushByOther; queue authentication is preserved existing behaviour (no rule).
    fn authenticity_witness() {
        let pushed = push(false,true); assert(pushed);
        let bot = push(true,true); assert(!bot);
        let stale = push(false,false); assert(!stale);
        let queue = check(true,true,true); assert(queue == Check::Queue);
        let spoof_id = check(true,false,true); assert(spoof_id == Check::Drop);
        let spoof_slug = check(true,true,false); assert(spoof_slug == Check::Drop);
        let other = check(false,true,true); assert(other == Check::Other);
    }
    /// Q52, docs/state-machine.md:720: head/PR filters and conditional look/wake have positive and negative witnesses.
    fn checks_witness() {
        let any = head_pull(true,true,false); assert(any);
        let listed = head_pull(true,false,true); assert(listed);
        let stale = head_pull(false,true,true); assert(!stale);
        let excluded = head_pull(true,false,false); assert(!excluded);
        let queue = check_action(true,false,false); assert(queue.look && !queue.wake);
        let pending = check_action(false,true,true); assert(pending.look && pending.wake);
        let stale = check_action(false,true,false); assert(!stale.look && !stale.wake);
        let none = check_action(false,false,true); assert(!none.look && !none.wake);
        let suite = suite_action(true); assert(suite.look && suite.wake);
        let idle = suite_action(false); assert(!idle.look && !idle.wake);
        let fresh = fresh_merge(true); assert(fresh);
        let open = fresh_merge(false); assert(!open);
    }
    }
}

pub mod projection {
    #[cfg(verus_keep_ghost)]
    use super::*;
    /// Preserved existing behaviour (no rule): PR-number matching is exact, including catch-up labels.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (left == right),))]
    pub fn number(left: u64, right: u64) -> bool {
        left == right
    }
    /// Q52 docs/state-table.md:543: stale-head missing-check facts skip node/incident lookup.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == same_head,))]
    pub fn missing_head(same_head: bool) -> bool {
        same_head
    }
    /// Preserved existing behaviour (no rule): only merged non-queue PRs are considered for catch-up bindings.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (merged && !queue_pr),))]
    pub fn catch_merged(merged: bool, queue_pr: bool) -> bool {
        merged && !queue_pr
    }
    /// Preserved existing behaviour (no rule): check routing includes closed unmerged nodes, while ordinary tracking excludes closed nodes.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!closed || include_closed && !merged),))]
    pub fn phase(closed: bool, include_closed: bool, merged: bool) -> bool {
        !closed || include_closed && !merged
    }
    /// Preserved existing behaviour (no rule): catch-up merged bindings concern closed nodes whose merge is not already recorded.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (bound && closed && unmerged),))]
    pub fn merged(bound: bool, closed: bool, unmerged: bool) -> bool {
        bound && closed && unmerged
    }
    /// Preserved existing behaviour (no rule): catch-up skips further person/head evidence after a queue PR or merged PR.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!queue_pr && !merged),))]
    pub fn catch_pull(queue_pr: bool, merged: bool) -> bool {
        !queue_pr && !merged
    }
    /// docs/state-table.md:667 PushByOther: an unknown observed head is recorded; known-head suppression preserves existing behaviour.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!known_head),))]
    pub fn unknown(known_head: bool) -> bool {
        !known_head
    }
    /// Preserved existing behaviour (no rule): hydrate every pending effect, then select only matching repo/PR enqueue effects.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (repository && number && enqueue),))]
    pub fn pending(repository: bool, number: bool, enqueue: bool) -> bool {
        repository && number && enqueue
    }
    /// Q52, docs/state-table.md:543: only a missing context on this head/node with an open incident is eligible for clearing.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (same_head && same_node && incident_open),))]
    pub fn missing(same_head: bool, same_node: bool, incident_open: bool) -> bool {
        same_head && same_node && incident_open
    }
    /// docs/state-table.md:669: catch-up person events belong to the tracked PR; close/dequeue mapping preserves existing behaviour.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (same_number && human),))]
    pub fn person(same_number: bool, human: bool) -> bool {
        same_number && human
    }

    #[cfg(verus_keep_ghost)]
    verus! {
    /// Preserved existing behaviour (no rule): include_closed, merged bindings and pending effect selection.
    fn bindings_witness() {
        let open = phase(false,false,false); assert(open);
        let closed = phase(true,false,false); assert(!closed);
        let late = phase(true,true,false); assert(late);
        let done = phase(true,true,true); assert(!done);
        let merge = merged(true,true,true); assert(merge);
        let active = merged(true,false,true); assert(!active);
        let ours = catch_pull(false,false); assert(ours);
        let queue = catch_pull(true,false); assert(!queue);
        let pending_ = pending(true,true,true); assert(pending_);
        let other = pending(true,true,false); assert(!other);
    }
    /// Q52 docs/state-table.md:543 and PushByOther :667: head/node/incident and unknown-head witnesses.
    fn evidence_witness() {
        let unknown_ = unknown(false); assert(unknown_);
        let known = unknown(true); assert(!known);
        let missing_ = missing(true,true,true); assert(missing_);
        let stale = missing(false,true,true); assert(!stale);
        let other = missing(true,false,true); assert(!other);
        let cleared = missing(true,true,false); assert(!cleared);
        let human = person(true,true); assert(human);
        let other_pr = person(false,true); assert(!other_pr);
        let bot = person(true,false); assert(!bot);
    }
    }
}

pub mod failure {
    #[cfg(verus_keep_ghost)]
    use super::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Failure<T> {
        Tests(T),
        Infrastructure(T),
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Evidence<T> {
        pub value: T,
        pub infra: bool,
    }
    /// docs/state-table.md:540-541,574-575: preserve the selected test/runner failure and its payload. Infrastructure's first check is selected at the typed boundary, as before (no rule).
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match failure {
    Some(Failure::Tests(value)) => matches!(out, Some(Evidence { value: v, infra: false }) if equal(v,value)),
    Some(Failure::Infrastructure(value)) => matches!(out, Some(Evidence { value: v, infra: true }) if equal(v,value)),
    None => matches!(out, None),
},))]
    pub fn evidence<T>(failure: Option<Failure<T>>) -> Option<Evidence<T>> {
        match failure {
            Some(Failure::Tests(value)) => Some(Evidence {
                value,
                infra: false,
            }),
            Some(Failure::Infrastructure(value)) => Some(Evidence { value, infra: true }),
            None => None,
        }
    }

    #[cfg(verus_keep_ghost)]
    verus! {
    /// docs/state-table.md:540-541: real test, infrastructure, absent and selected-payload witnesses.
    fn failure_witness() {
        let tests = evidence(Some(Failure::Tests(7u64))); assert(matches!(tests,Some(Evidence {value:7,infra:false})));
        let infra = evidence(Some(Failure::Infrastructure(11u64))); assert(matches!(infra,Some(Evidence {value:11,infra:true})));
        let none = evidence::<u64>(None); assert(none.is_none());
    }
    }
}
