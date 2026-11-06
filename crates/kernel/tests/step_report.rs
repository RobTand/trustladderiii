use tl3_kernel::*;

const HEAD: HeadId = HeadId(7);
const PASS: Reply = Reply::ChecksDone {
    report: true,
    pass: true,
    in_footprint: true,
};

struct Run {
    world: World,
    report: Report,
    command: u64,
}

impl Run {
    fn new() -> Self {
        let mut run = Self {
            world: World::new(&[1], 1),
            report: Report::new(),
            command: 0,
        };
        run.send(Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        });
        run.confirm();
        run
    }

    fn send(&mut self, event: Event) {
        let result = step(self.world.clone(), event);
        assert!(result.world.valid(), "invalid world after {event:?}");
        self.world = result.world;
        self.report = result.report;
    }

    fn confirm(&mut self) {
        for _ in 0..64 {
            let Some(effect) = self.world.nodes[0].effect else {
                return;
            };
            self.send(Event::Ack {
                node: NodeId(0),
                seq: effect.seq,
                ack: Ack::Confirmed,
            });
        }
        panic!("effects did not settle");
    }

    fn reply(&mut self, reply: Reply) {
        self.confirm();
        self.send(Event::Reply {
            node: NodeId(0),
            generation: self.world.nodes[0].generation,
            reply,
        });
    }

    fn command(&mut self, command: Command) {
        self.command += 1;
        self.send(Event::Command {
            node: NodeId(0),
            id: self.command,
            command,
        });
    }

    fn fact(&mut self, fact: Fact) {
        self.send(Event::Fact {
            node: NodeId(0),
            fact,
        });
    }

    fn making(&mut self) {
        self.reply(Reply::IngestDone { size: 1 });
        self.reply(Reply::AuditAgrees);
        let footprint = self.world.intern_footprint([FileId(0)]);
        self.reply(Reply::PlanLeaf {
            footprint,
            fits: true,
        });
        self.confirm();
        assert_eq!(self.world.nodes[0].phase, Phase::Making);
    }

    fn checking(&mut self) {
        self.making();
        self.reply(Reply::MakerDone { head: HEAD });
    }

    fn reviewing(&mut self) {
        self.checking();
        self.reply(PASS);
    }

    fn queued(&mut self) {
        self.reviewing();
        self.reply(Reply::Approved { head: HEAD });
        self.confirm();
        assert_eq!(self.world.nodes[0].phase, Phase::Queued);
    }

    fn failed(&self, cause: FailedRoundCause) {
        assert!(self.report.entries.contains(&ReportEntry::FailedRound {
            node: NodeId(0),
            cause,
        }));
    }

    fn stopped(&self, cause: StopCause) {
        assert!(self.report.entries.contains(&ReportEntry::Stopped {
            node: NodeId(0),
            cause,
        }));
    }
}

#[test]
fn failed_checks_report_the_round() {
    let mut run = Run::new();
    run.checking();
    run.reply(Reply::ChecksDone {
        report: true,
        pass: false,
        in_footprint: true,
    });
    run.failed(FailedRoundCause::ChecksFailed);
}

#[test]
fn requested_changes_report_the_round() {
    let mut run = Run::new();
    run.reviewing();
    run.reply(Reply::ChangesRequested);
    run.failed(FailedRoundCause::ChangesRequested);
}

#[test]
fn maker_cant_meet_reports_the_existing_ladder_path() {
    let mut run = Run::new();
    run.making();
    run.reply(Reply::MakerCantMeet);
    run.failed(FailedRoundCause::MakerCantMeet);
    assert_eq!(run.world.nodes[0].round, 0);
    assert_eq!(run.world.nodes[0].attempt, 2);
}

#[test]
fn ci_failure_reports_only_a_work_failure() {
    let mut run = Run::new();
    run.queued();
    let mut infra = Run {
        world: run.world.clone(),
        report: Report::new(),
        command: 0,
    };
    infra.fact(Fact::CiFailed { infra: true });
    assert!(infra.report.entries.is_empty());
    run.fact(Fact::CiFailed { infra: false });
    assert_eq!(
        run.report.entries,
        vec![ReportEntry::FailedRound {
            node: NodeId(0),
            cause: FailedRoundCause::CiFailed,
        }]
    );
}

#[test]
fn ci_failure_outside_the_failed_round_phases_has_no_entry() {
    let mut run = Run::new();
    run.making();
    for infra in [false, true] {
        let before = run.world.clone();
        run.fact(Fact::CiFailed { infra });
        assert_eq!(run.world, before);
        assert!(run.report.entries.is_empty());
    }
}

#[test]
fn failed_run_reports_the_operation_failure() {
    let mut run = Run::new();
    run.making();
    run.reply(Reply::RunFailed);
    run.failed(FailedRoundCause::RunFailed);
}

#[test]
fn stop_and_replacement_report_their_own_causes() {
    for (command, cause) in [
        (Command::Stop, StopCause::UserStop),
        (Command::ChangeRequest, StopCause::Replaced),
    ] {
        let mut run = Run::new();
        run.command(command);
        run.stopped(cause);
        run.command(Command::Stop);
        assert!(run.report.entries.is_empty());
        run.confirm();
        assert!(run.report.entries.is_empty());
    }
}

#[test]
fn pr_closed_is_distinct_from_a_user_stop() {
    let mut run = Run::new();
    run.queued();
    run.fact(Fact::PrClosedByPerson);
    run.stopped(StopCause::PrClosed);
    assert!(!run.report.entries.contains(&ReportEntry::Stopped {
        node: NodeId(0),
        cause: StopCause::UserStop,
    }));
}

#[test]
fn merge_reports_the_stop_even_when_settle_closes_immediately() {
    let mut run = Run::new();
    run.queued();
    run.fact(Fact::PrMerged {
        head: HEAD,
        checks_passed: true,
    });
    run.stopped(StopCause::Merged);
}

#[test]
fn early_end_reports_each_existing_cause() {
    for (early, cause) in [
        (EarlyEnd::Duplicate, StopCause::Duplicate),
        (EarlyEnd::AlreadyDone, StopCause::AlreadyDone),
        (EarlyEnd::NotActionable, StopCause::NotActionable),
    ] {
        let mut run = Run::new();
        run.reply(Reply::IngestEarlyEnd(early));
        run.reply(Reply::AuditAgrees);
        run.stopped(cause);
    }
}

#[test]
fn review_stop_keeps_the_failing_step() {
    let mut run = Run::new();
    run.making();
    run.world.fail_fast = true;
    run.reply(Reply::MakerCantMeet);
    run.failed(FailedRoundCause::MakerCantMeet);
    run.stopped(StopCause::ReviewStop(ReviewStop::MakerCantMeet));
}

#[test]
fn dequeue_pauses_without_a_stop_report() {
    let mut run = Run::new();
    run.queued();
    run.fact(Fact::PrDequeuedByPerson);
    assert_eq!(run.world.nodes[0].phase, Phase::Ready(Resume::Publishing));
    assert!(run.report.entries.is_empty());
}

#[test]
fn stale_admission_reports_the_reply_kind_without_changing_the_node() {
    let mut run = Run::new();
    run.making();
    let before = run.world.nodes[0];
    run.send(Event::Reply {
        node: NodeId(0),
        generation: before.generation - 1,
        reply: Reply::MakerDone { head: HEAD },
    });
    assert_eq!(run.world.nodes[0], before);
    assert_eq!(
        run.report.entries,
        [ReportEntry::Stale {
            node: NodeId(0),
            reply_kind: ReplyKind::MakerDone,
        }]
    );
}

#[test]
fn checked_step_refuses_before_any_change() {
    let mut run = Run::new();
    run.making();
    run.world.nodes[0].generation = u64::MAX;
    let before = run.world.clone();
    let refusal = match step_checked(
        run.world,
        Event::Command {
            node: NodeId(0),
            id: 1,
            command: Command::Stop,
        },
    ) {
        Ok(_) => panic!("exhausted world was accepted"),
        Err(refusal) => refusal,
    };
    assert_eq!(refusal.world, before);
    assert_eq!(
        refusal.exhaustion,
        CounterExhaustion {
            node: Some(NodeId(0)),
            counter: "generation",
        }
    );
}

#[test]
fn checked_step_returns_the_same_success_as_step() {
    let run = Run::new();
    let event = Event::Command {
        node: NodeId(0),
        id: 1,
        command: Command::Stop,
    };
    let direct = step(run.world.clone(), event);
    let checked = match step_checked(run.world, event) {
        Ok(result) => result,
        Err(_) => panic!("world has headroom"),
    };
    assert_eq!(checked.world, direct.world);
    assert_eq!(checked.outputs, direct.outputs);
    assert_eq!(checked.report, direct.report);
}
