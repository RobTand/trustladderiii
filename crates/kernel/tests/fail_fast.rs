//! Q54 (#440, Rob, 2026-11-02): with `fail_fast`, every step that would retry
//! the work stops the request for TL3 review instead. One row per line of the
//! issue's table, plus the default, which keeps today's rules.

use tl3_kernel::*;

const CHECKS_PASS: Reply = Reply::ChecksDone {
    report: true,
    pass: true,
    in_footprint: true,
};
const CHECKS_FAIL: Reply = Reply::ChecksDone {
    report: true,
    pass: false,
    in_footprint: true,
};

struct Run {
    w: World,
    notices: Vec<Notice>,
    head: u64,
}

impl Run {
    fn new(fail_fast: bool) -> Run {
        let mut w = World::new(&[1], 1);
        w.fail_fast = fail_fast;
        let mut run = Run {
            w,
            notices: Vec::new(),
            head: 0,
        };
        run.send(Event::NewRequest { repo: RepoId(0) });
        run.confirm();
        run
    }

    fn send(&mut self, event: Event) {
        let (w, out) = step(self.w.clone(), event);
        assert!(w.valid(), "invalid world after {event:?}:\n{w:#?}");
        for o in out {
            if let Out::Notice { notice, .. } = o {
                self.notices.push(notice);
            }
        }
        self.w = w;
    }

    fn node(&self) -> Node {
        self.w.nodes[0]
    }

    fn phase(&self) -> Phase {
        self.node().phase
    }

    fn confirm(&mut self) {
        for _ in 0..64 {
            let Some(e) = self.node().effect else { return };
            self.send(Event::Ack {
                node: NodeId(0),
                seq: e.seq,
                ack: Ack::Confirmed,
            });
        }
        panic!("effects never stopped");
    }

    fn reply(&mut self, reply: Reply) {
        self.confirm();
        let generation = self.node().generation;
        self.send(Event::Reply {
            node: NodeId(0),
            generation,
            reply,
        });
        self.confirm();
    }

    fn fact(&mut self, fact: Fact) {
        self.send(Event::Fact {
            node: NodeId(0),
            fact,
        });
        self.confirm();
    }

    fn plan(&mut self) {
        let footprint = self.w.intern_footprint([FileId(0)]);
        self.reply(Reply::PlanLeaf {
            footprint,
            fits: true,
        });
    }

    fn start_planning(&mut self) {
        self.reply(Reply::IngestDone { size: 1 });
        self.reply(Reply::AuditAgrees);
        assert!(matches!(self.phase(), Phase::Planning { .. }));
    }

    fn start_making(&mut self) {
        self.start_planning();
        self.plan();
        assert_eq!(self.phase(), Phase::Making);
    }

    fn made(&mut self) {
        self.head += 1;
        self.reply(Reply::MakerDone {
            head: HeadId(self.head),
        });
        assert_eq!(self.phase(), Phase::Checking);
    }

    fn reviewed(&mut self) {
        self.made();
        self.reply(CHECKS_PASS);
        assert_eq!(self.phase(), Phase::Reviewing);
    }

    fn queued(&mut self) {
        self.reviewed();
        self.reply(Reply::Approved {
            head: HeadId(self.head),
        });
        assert_eq!(self.phase(), Phase::Queued);
    }

    /// Round 0 fails review, so the free fix round starts.
    fn fix_round(&mut self) {
        self.reviewed();
        self.reply(Reply::ChangesRequested);
        assert_eq!(self.phase(), Phase::Making);
        assert_eq!((self.node().attempt, self.node().round), (1, 1));
    }

    fn answer(&mut self, answer: Answer) {
        let q = self.node().question.expect("an open question");
        self.send(Event::Command {
            node: NodeId(0),
            id: self.node().last_cmd + 1,
            command: Command::Answer {
                question: q.id,
                answer,
            },
        });
        self.confirm();
    }

    fn spent(&self) -> u32 {
        self.node().credit.spent
    }

    fn asked_rob(&self) -> bool {
        self.notices.iter().any(|n| matches!(n, Notice::Ask { .. }))
    }

    /// Stopped for TL3 review: no credit, no question, the attempt unchanged,
    /// and teardown ends in Closed(Stopped) with the summary.
    fn assert_stopped_for_review(&mut self, step: ReviewStop, spent: u32, attempt: u8) {
        assert_eq!(self.phase(), Phase::Closed(Outcome::Stopped));
        let n = self.node();
        assert_eq!(n.review_stop, Some(step));
        assert_eq!(
            n.credit.spent, spent,
            "a stop for TL3 review spends nothing"
        );
        assert_eq!(n.attempt, attempt, "no new attempt");
        assert!(!n.replanned, "no re-plan");
        assert!(n.question.is_none());
        assert!(!self.asked_rob(), "a stop for TL3 review asks Rob nothing");
        assert!(self.notices.contains(&Notice::Summary(Outcome::Stopped)));
    }
}

#[test]
fn a_failed_round_zero_keeps_its_free_fix_round() {
    let mut run = Run::new(true);
    run.start_making();
    run.made();
    run.reply(CHECKS_FAIL);
    assert_eq!(run.phase(), Phase::Making);
    assert_eq!((run.node().attempt, run.node().round), (1, 1));
    assert_eq!(run.node().review_stop, None);
}

#[test]
fn failed_checks_in_round_one_stop_for_tl3_review() {
    let mut run = Run::new(true);
    run.start_making();
    run.fix_round();
    let spent = run.spent();
    run.made();
    run.reply(CHECKS_FAIL);
    run.assert_stopped_for_review(ReviewStop::ChecksFailed, spent, 1);
}

#[test]
fn changes_requested_in_round_one_stop_for_tl3_review() {
    let mut run = Run::new(true);
    run.start_making();
    run.fix_round();
    let spent = run.spent();
    run.reviewed();
    run.reply(Reply::ChangesRequested);
    run.assert_stopped_for_review(ReviewStop::ChangesRequested, spent, 1);
}

#[test]
fn ci_failing_on_the_code_in_round_one_stops_for_tl3_review() {
    let mut run = Run::new(true);
    run.start_making();
    run.queued();
    run.fact(Fact::CiFailed { infra: false });
    assert_eq!(
        run.phase(),
        Phase::Making,
        "round 0's fix round is unchanged"
    );
    let spent = run.spent();
    run.queued();
    run.fact(Fact::CiFailed { infra: false });
    run.assert_stopped_for_review(ReviewStop::CiFailed, spent, 1);
}

#[test]
fn maker_cant_meet_stops_for_tl3_review_in_any_round() {
    let mut run = Run::new(true);
    run.start_making();
    let spent = run.spent();
    run.reply(Reply::MakerCantMeet);
    run.assert_stopped_for_review(ReviewStop::MakerCantMeet, spent, 1);
}

#[test]
fn maker_out_of_room_stops_instead_of_splitting() {
    let mut run = Run::new(true);
    run.start_making();
    let spent = run.spent();
    run.reply(Reply::MakerOutOfRoom);
    run.assert_stopped_for_review(ReviewStop::MakerOutOfRoom, spent, 1);
}

#[test]
fn maker_needing_files_that_do_not_fit_stops_instead_of_splitting() {
    let mut run = Run::new(true);
    run.start_making();
    let spent = run.spent();
    let files = run.w.intern_footprint([FileId(1)]);
    run.reply(Reply::MakerNeedsFiles { files, fits: false });
    run.assert_stopped_for_review(ReviewStop::MakerNeedsFiles, spent, 1);
}

#[test]
fn a_footprint_extension_that_fits_is_unchanged() {
    let mut run = Run::new(true);
    run.start_making();
    let files = run.w.intern_footprint([FileId(1)]);
    run.reply(Reply::MakerNeedsFiles { files, fits: true });
    assert_eq!(run.phase(), Phase::Making);
    assert_eq!(run.node().review_stop, None);
}

/// fail_fast stops retries, not credit decisions: a footprint extension that
/// fits but has no credit asks Rob for credit, as without the flag (#482).
#[test]
fn a_footprint_extension_that_fits_without_credit_asks_rob_for_credit() {
    let mut run = Run::new(true);
    run.start_making();
    let mut world = run.w.clone();
    world.nodes[0].credit.spent = world.nodes[0].credit.start;
    run.w = world;
    assert_eq!(run.node().credit.balance(), 0);
    let files = run.w.intern_footprint([FileId(1)]);
    run.reply(Reply::MakerNeedsFiles { files, fits: true });
    assert_eq!(run.phase(), Phase::AwaitingRob);
    let q = run.node().question.unwrap();
    assert_eq!(
        (q.kind, q.cont, q.cost),
        (QKind::OutOfCredit, Cont::Ready(Resume::Making), 1)
    );
    assert_eq!(run.node().review_stop, None);
    assert!(!run.node().holds);
    run.answer(Answer::AddCredit(1));
    assert_eq!(run.phase(), Phase::Making);
    let footprint: Vec<_> = run.w.files(run.node().footprint).unwrap().collect();
    assert!(
        footprint.contains(&FileId(1)),
        "the extension applies once funded"
    );
}

#[test]
fn reviewer_out_of_room_keeps_one_fresh_review_then_stops() {
    let mut run = Run::new(true);
    run.start_making();
    run.reviewed();
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::Reviewing, "Q44's fresh review stays");
    let spent = run.spent();
    run.reply(Reply::ReviewerOutOfRoom);
    run.assert_stopped_for_review(ReviewStop::ReviewerOutOfRoom, spent, 1);
}

#[test]
fn plan_cant_meet_stops_without_rerunning_the_planner() {
    let mut run = Run::new(true);
    run.start_planning();
    let generation = run.node().generation;
    run.reply(Reply::PlanCantMeet);
    assert!(run.node().generation > generation);
    run.assert_stopped_for_review(ReviewStop::PlanCantMeet, 0, 0);
}

/// The planner worker reports any broken rule as `cant_meet`, so a rule
/// break reaches the kernel as `PlanCantMeet` and stops at once. The kernel's
/// own rule-break path, `PlanLeaf` that doesn't fit, can't arise live, and
/// fail_fast leaves it as it was (#482 review).
#[test]
fn the_kernels_rule_break_path_is_unchanged_under_fail_fast() {
    let mut run = Run::new(true);
    run.start_planning();
    let footprint = run.w.intern_footprint([FileId(0)]);
    run.reply(Reply::PlanLeaf {
        footprint,
        fits: false,
    });
    assert!(matches!(run.phase(), Phase::Planning { .. }));
    run.reply(Reply::PlanLeaf {
        footprint,
        fits: false,
    });
    assert_eq!(run.phase(), Phase::AwaitingRob);
    assert_eq!(run.node().question.unwrap().kind, QKind::CantMeet);
    assert_eq!(run.node().review_stop, None);
}

#[test]
fn infrastructure_failures_keep_their_retries_and_incidents() {
    let mut run = Run::new(true);
    run.start_making();
    run.reply(Reply::RunFailed);
    assert_eq!(run.phase(), Phase::Making);
    run.reply(Reply::RunFailed);
    assert!(run.node().waiting_incident, "Q7 is unchanged");
    assert_eq!(run.node().review_stop, None);
}

#[test]
fn a_new_world_keeps_todays_rules() {
    assert!(!World::new(&[1], 1).fail_fast);
}

#[test]
fn without_fail_fast_round_one_still_starts_the_next_attempt() {
    let mut run = Run::new(false);
    run.start_making();
    run.fix_round();
    let spent = run.spent();
    run.reviewed();
    run.reply(Reply::ChangesRequested);
    assert_eq!(run.phase(), Phase::Making);
    assert_eq!((run.node().attempt, run.node().round), (2, 0));
    assert_eq!(run.spent(), spent + 1);
    assert_eq!(run.node().review_stop, None);
}

#[test]
fn without_fail_fast_plan_cant_meet_still_reruns_the_planner() {
    let mut run = Run::new(false);
    run.start_planning();
    run.reply(Reply::PlanCantMeet);
    assert!(matches!(run.phase(), Phase::Planning { .. }));
    assert_eq!(run.node().review_stop, None);
}

#[test]
fn without_fail_fast_out_of_room_still_splits() {
    let mut run = Run::new(false);
    run.start_making();
    run.reply(Reply::MakerOutOfRoom);
    assert_eq!(run.phase(), Phase::Planning { must_split: true });
    assert_eq!(run.node().review_stop, None);
}
