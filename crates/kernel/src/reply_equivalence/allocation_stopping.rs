//! #806: full-Node parity for the standing-rule allocation `stop_node` extraction.
use super::*;

fn compare_stopping(node: Node, outcome: Outcome) {
    let mut actual = node;
    let mut expected = node;
    let actual_ok = catch_unwind(AssertUnwindSafe(|| enter_stopping(&mut actual, outcome))).is_ok();
    let expected_ok = catch_unwind(AssertUnwindSafe(|| {
        stopping_reference::reference_enter_stopping(&mut expected, outcome);
    }))
    .is_ok();
    assert_eq!(
        actual_ok, expected_ok,
        "stop panic parity for {node:?}, {outcome:?}"
    );
    assert_eq!(
        actual, expected,
        "stop full-node parity for {node:?}, {outcome:?}"
    );
}

const OUTCOMES: [Outcome; 6] = [
    Outcome::Merged,
    Outcome::Stopped,
    Outcome::Replaced,
    Outcome::NotActionable,
    Outcome::Duplicate,
    Outcome::AlreadyDone,
];

/// #806 standing rule: every phase and outcome, generation edge, and effect/question branch.
#[test]
fn every_stopping_branch_matches() {
    let mut comparisons = 0;
    for phase in phases() {
        for outcome in OUTCOMES {
            for generation in [0, 7, u64::MAX - 1, u64::MAX] {
                for flags in 0u8..8 {
                    let mut node = Node::new(RepoId(1));
                    node.phase = phase;
                    node.generation = generation;
                    node.waiting_incident = flags & 1 != 0;
                    node.incident_needs_rob = flags & 2 != 0;
                    node.effect = (flags & 4 != 0).then_some(InFlight {
                        seq: 3,
                        kind: EffectKind::ClosePr,
                        cancelling: false,
                    });
                    node.question = Some(Question {
                        id: 9,
                        kind: QKind::OutOfCredit,
                        cont: Cont::Nowhere,
                        cost: 0,
                        clarified: false,
                    });
                    compare_stopping(node, outcome);
                    comparisons += 1;
                }
            }
        }
    }
    assert_eq!(comparisons, 4416);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]
    /// #806 standing rule: arbitrary full Nodes (counters, ownership, effects, questions)
    /// under all six outcomes. Same node domain and case count as the former mixed test.
    #[test]
    fn arbitrary_stopping_state_matches(raw in any::<[u8; 64]>(), wide in any::<[u64; 8]>()) {
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
        for outcome in OUTCOMES {
            compare_stopping(node, outcome);
        }
    }
}
