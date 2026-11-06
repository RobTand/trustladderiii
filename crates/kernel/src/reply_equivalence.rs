//! #806: full-state parity for the standing-rule accepted-reply extraction.
use super::*;
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

fn replies(head: HeadId, files: Footprint, bits: u8) -> [Reply; 21] {
    [
        Reply::IngestDone { size: bits },
        Reply::IngestEarlyEnd(EarlyEnd::Duplicate),
        Reply::AuditAgrees,
        Reply::AuditDisagrees,
        Reply::PlanLeaf {
            footprint: files,
            fits: bits & 1 != 0,
        },
        Reply::PlanSplit,
        Reply::PlanCantMeet,
        Reply::PlanCheckRejected,
        Reply::PlanCheckNeedsSplit,
        Reply::ProtectedPaths,
        Reply::MakerDone { head },
        Reply::MakerNeedsFiles {
            files,
            fits: bits & 2 != 0,
        },
        Reply::MakerCantMeet,
        Reply::MakerOutOfRoom,
        Reply::ChecksDone {
            report: bits & 4 != 0,
            pass: bits & 8 != 0,
            in_footprint: bits & 16 != 0,
        },
        Reply::Approved { head },
        Reply::ChangesRequested,
        Reply::ReviewerOutOfRoom,
        Reply::RebaseClean { head },
        Reply::RebaseNeedsChanges,
        Reply::RunFailed,
    ]
}

fn compare(world: &World, i: usize, generation: u64, reply: Reply, outputs: Outbox) {
    let mut actual = world.clone();
    let mut expected = world.clone();
    let mut previous = world.clone();
    let mut actual_out = outputs;
    let mut expected_out = outputs;
    let mut previous_out = outputs;
    let actual_ok = catch_unwind(AssertUnwindSafe(|| {
        on_reply(&mut actual, i, generation, reply, &mut actual_out);
    }))
    .is_ok();
    let expected_ok = catch_unwind(AssertUnwindSafe(|| {
        reference::reference_on_reply(&mut expected, i, generation, reply, &mut expected_out);
    }))
    .is_ok();
    let previous_ok = catch_unwind(AssertUnwindSafe(|| {
        reply_admission::stopping_reference::reference_on_reply(
            &mut previous,
            i,
            generation,
            reply,
            &mut previous_out,
        );
    }))
    .is_ok();
    assert_eq!(
        actual_ok, previous_ok,
        "stopping extraction panic parity for {reply:?}"
    );
    assert_eq!(
        actual, previous,
        "stopping extraction world parity for {reply:?}"
    );
    assert_eq!(
        actual_out, previous_out,
        "stopping extraction output parity for {reply:?}"
    );
    assert_eq!(actual_ok, expected_ok, "panic parity for {reply:?}");
    assert_eq!(actual, expected, "world parity for {reply:?}");
    assert_eq!(actual_out, expected_out, "all output slots for {reply:?}");
}

/// #806 standing rule: every phase, reply variant and admission branch is exercised.
#[test]
fn every_reply_and_admission_branch_matches() {
    let mut comparisons = 0;
    for phase in phases() {
        for fast in [false, true] {
            for mode in 0..5 {
                let mut world = World::new(&[3, 2], 2);
                let mut node = Node::new(RepoId(0));
                node.phase = phase;
                node.pos = if phase == Phase::Making { 2 } else { 1 };
                node.generation = 11;
                node.run = if mode == 1 { None } else { Some(11) };
                node.job = if mode == 2 { None } else { Some(11) };
                node.pool = mode != 1;
                node.holds = phase.holding();
                node.footprint = Footprint::inline(1);
                node.head = Some(HeadId(7));
                node.checked = node.head;
                node.credit.ready = true;
                node.credit.start = 8;
                node.run_failures = u8::from(mode == 3);
                node.rechecks = if mode == 3 { FREE_RECHECKS } else { 0 };
                node.review_restarted = mode == 4;
                world.nodes.push(node);
                world.nodes.push(Node::new(RepoId(1)));
                world.fail_fast = fast;
                let generation = if mode == 4 { 12 } else { 11 };
                for reply in replies(HeadId(7), Footprint::inline(2), 31) {
                    compare(&world, 0, generation, reply, Outbox::new());
                    comparisons += 1;
                }
            }
        }
    }
    assert_eq!(comparisons, 4830);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]
    /// #806 standing rule: arbitrary payloads, counters, ownership and existing outputs.
    #[test]
    fn arbitrary_reply_state_matches(raw in any::<[u8; 64]>(), wide in any::<[u64; 8]>()) {
        let mut world = World::new(&[1 + raw[0] % 3, 1 + raw[1] % 3], 1 + raw[2] % 2);
        let foreign = {
            let mut other = World::new(&[1], 1);
            other.intern_footprint([FileId(64), FileId(wide[0])])
        };
        let files = if raw[3] & 1 == 0 { Footprint::inline(wide[0]) } else { foreign };
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
        let reply = replies(HeadId(wide[4]), files, raw[61])[raw[62] as usize % 21];
        let generation = if raw[63] & 1 != 0 { wide[1] } else { wide[2] };
        compare(&world, 0, generation, reply, outputs);
    }
}
