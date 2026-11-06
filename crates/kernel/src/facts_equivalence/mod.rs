//! #806: full-state parity for the standing-rule fact dispatch extraction.
use crate::*;
use proptest::prelude::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

mod reference;

fn phases() -> [Phase; 23] {
    [
        Phase::Ingesting,
        Phase::Auditing,
        Phase::Unplanned,
        Phase::Planning { must_split: false },
        Phase::Planning { must_split: true },
        Phase::Ready(Resume::Making),
        Phase::Ready(Resume::Checking),
        Phase::Ready(Resume::Reviewing),
        Phase::Ready(Resume::Publishing),
        Phase::Ready(Resume::Rebasing),
        Phase::Making,
        Phase::Checking,
        Phase::Reviewing,
        Phase::Publishing,
        Phase::Queued,
        Phase::Rebasing,
        Phase::AwaitingRob,
        Phase::Stopping(Outcome::Stopped),
        Phase::Stopping(Outcome::Merged),
        Phase::Stopping(Outcome::Replaced),
        Phase::Closed(Outcome::Stopped),
        Phase::Closed(Outcome::Merged),
        Phase::Closed(Outcome::Replaced),
    ]
}

/// Every fact routed through fact_transition, including the dispatcher's no-op arms.
fn facts(head: HeadId) -> [Fact; 15] {
    [
        Fact::PrClosedByPerson,
        Fact::PrDequeuedByPerson,
        Fact::PushByOther { head },
        Fact::CiFailed { infra: false },
        Fact::CiFailed { infra: true },
        Fact::Conflict,
        Fact::IncidentCleared { by_rob: false },
        Fact::IncidentCleared { by_rob: true },
        Fact::UpstreamStalled,
        Fact::Priority(Priority::P0),
        Fact::Priority(Priority::P1),
        Fact::Priority(Priority::P2),
        Fact::Priority(Priority::P3),
        Fact::Priority(Priority::Unranked),
        Fact::PrMerged {
            head,
            checks_passed: true,
        },
    ]
}

fn compare(world: &World, i: usize, fact: Fact, outputs: Outbox) {
    let mut actual = world.clone();
    let mut expected = world.clone();
    let mut actual_out = outputs;
    let mut expected_out = outputs;
    let actual_ok = catch_unwind(AssertUnwindSafe(|| {
        fact_transition(&mut actual, i, fact, &mut actual_out);
    }))
    .is_ok();
    let expected_ok = catch_unwind(AssertUnwindSafe(|| {
        reference::fact_transition(&mut expected, i, fact, &mut expected_out);
    }))
    .is_ok();
    assert_eq!(actual_ok, expected_ok, "panic parity for {fact:?}");
    assert_eq!(actual, expected, "world parity for {fact:?}");
    assert_eq!(actual_out, expected_out, "all output slots for {fact:?}");
}

/// #806 standing rule: every phase, fact variant and arm branch is exercised.
#[test]
fn every_fact_and_dispatch_branch_matches() {
    let mut comparisons = 0;
    for phase in phases() {
        for fast in [false, true] {
            for mode in 0..6u8 {
                let mut world = World::new(&[3, 2], 2);
                let mut node = Node::new(RepoId(0));
                node.phase = phase;
                node.pos = if phase == Phase::Making { 2 } else { 1 };
                node.generation = 11;
                node.run = (mode != 1).then_some(11);
                node.pool = mode != 1;
                node.holds = phase.holding();
                node.footprint = Footprint::inline(1);
                node.head = Some(HeadId(7));
                node.checked = node.head;
                node.credit.ready = true;
                node.credit.start = 8;
                node.had_pr = mode != 2;
                node.pr_open = mode != 3;
                node.waiting_incident = mode == 4;
                node.incident_needs_rob = mode == 4;
                node.run_incidents = if mode == 5 { 1 } else { 0 };
                node.run_failures = u8::from(mode == 5);
                node.rechecks = if mode == 5 { FREE_RECHECKS } else { 0 };
                node.round = mode % 2;
                world.nodes.push(node);
                world.nodes.push(Node::new(RepoId(1)));
                world.fail_fast = fast;
                for fact in facts(HeadId(9)) {
                    compare(&world, 0, fact, Outbox::new());
                    comparisons += 1;
                }
            }
        }
    }
    assert_eq!(comparisons, 23 * 2 * 6 * 15);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]
    /// #806 standing rule: arbitrary payloads, counters, ownership and existing outputs.
    #[test]
    fn arbitrary_fact_state_matches(raw in any::<[u8; 64]>(), wide in any::<[u64; 8]>()) {
        let mut world = World::new(&[1 + raw[0] % 3, 1 + raw[1] % 3], 1 + raw[2] % 2);
        let mut node = Node::new(RepoId((raw[4] % 2) as u64));
        node.phase = phases()[raw[5] as usize % phases().len()];
        node.pos = raw[6]; node.generation = wide[1]; node.next_seq = wide[2];
        node.run = (raw[7] & 1 != 0).then_some(wide[1]);
        node.job = (raw[8] & 1 != 0).then_some(wide[1]);
        node.pool = raw[9] & 1 != 0; node.holds = raw[10] & 1 != 0;
        node.paused = raw[11] & 1 != 0; node.last_cmd = wide[3];
        node.attempt = raw[12]; node.round = raw[13]; node.replanned = raw[14] & 1 != 0;
        node.plan_retries = raw[15]; node.audit_rounds = raw[16];
        node.run_failures = raw[17]; node.run_incidents = raw[18];
        node.fx_failures = raw[19]; node.fx_incidents = raw[20];
        node.waiting_incident = raw[21] & 1 != 0; node.incident_needs_rob = raw[22] & 1 != 0;
        node.head = (raw[23] & 1 != 0).then_some(HeadId(wide[4]));
        node.checked = (raw[24] & 1 != 0).then_some(HeadId(wide[4]));
        node.approved = (raw[25] & 1 != 0).then_some(HeadId(wide[5]));
        node.merged = (raw[26] & 1 != 0).then_some(HeadId(wide[6]));
        node.pr_open = raw[27] & 1 != 0; node.had_pr = raw[28] & 1 != 0;
        node.worktree = raw[29] & 1 != 0; node.rechecks = raw[30];
        node.review_restarted = raw[31] & 1 != 0;
        node.credit.ready = raw[32] & 1 != 0; node.credit.withheld = raw[33] & 1 != 0;
        node.credit.start = wide[5]; node.credit.spent = wide[6];
        node.credit.earned = raw[34] as u64; node.credit.added = raw[35] as u64;
        node.credit.merge_credited = raw[36] & 1 != 0;
        node.proposed = (raw[37] & 1 != 0).then_some(HeadId(wide[7]));
        node.adopted = raw[38] & 1 != 0; node.planning_started = raw[39] & 1 != 0;
        node.waiting_prerequisite = (raw[40] & 1 != 0).then_some(wide[0]);
        node.footprint = Footprint::inline(wide[3]);
        node.ticket = (raw[41] & 1 != 0).then_some(wide[2]);
        node.effect = (raw[42] & 1 != 0).then_some(InFlight { seq: wide[3],
            kind: EffectKind::StartRun(Role::Review), cancelling: raw[43] & 1 != 0 });
        if raw[44] & 1 != 0 {
            node.question = Some(Question { id: wide[0], kind: QKind::OutOfCredit,
                cont: Cont::Ready(Resume::Making), cost: raw[45], clarified: raw[46] & 1 != 0 });
        }
        if raw[47] & 1 != 0 { node.review_stop = Some(ReviewStop::ChangesRequested); }
        if raw[48] & 1 != 0 {
            node.pos = if node.phase == Phase::Making { 2 } else { 1 };
        }
        world.nodes.push(node); world.nodes.push(Node::new(RepoId(1)));
        world.next_question = wide[7]; world.next_ticket = wide[0];
        world.fail_fast = raw[49] & 1 != 0;
        let mut outputs = Outbox::new();
        outputs.len = raw[50] as usize % (OUTBOX + 1);
        for (slot, item) in outputs.items.iter_mut().enumerate() {
            if raw[51 + slot] & 1 != 0 {
                *item = Some(Out::Notice { node: NodeId(wide[slot]),
                    notice: Notice::Incident { needs_rob: raw[56 + slot] & 1 != 0 } });
            }
        }
        let fact = facts(HeadId(wide[4] ^ u64::from(raw[61])))[raw[62] as usize % 15];
        compare(&world, 0, fact, outputs);
    }
}
