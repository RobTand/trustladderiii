//! Scripted runs through the slice-one kernel. Every step checks that the
//! world stays valid and settled.

use std::collections::BTreeSet;
use tl3_kernel::*;

const CHECKS_PASS: Reply = Reply::ChecksDone {
    report: true,
    pass: true,
    in_footprint: true,
};

struct Run {
    w: World,
    cmd: u64,
    seen: BTreeSet<&'static str>,
    notices: Vec<Notice>,
}

impl Run {
    fn new() -> Run {
        let mut run = Run {
            w: World::new(&[1, 1], 1),
            cmd: 0,
            seen: BTreeSet::new(),
            notices: Vec::new(),
        };
        run.send(Event::NewRequest { repo: RepoId(0) });
        run.confirm();
        run
    }

    fn send(&mut self, event: Event) {
        let (w, out) = step(self.w.clone(), event);
        assert!(w.valid(), "invalid world after {event:?}:\n{w:#?}");
        assert!(w.is_settled(), "unsettled world after {event:?}");
        for o in out {
            if let Out::Notice { notice, .. } = o {
                self.notices.push(notice);
            }
        }
        for n in &w.nodes {
            self.seen.insert(n.phase.name());
        }
        self.w = w;
    }

    fn node(&self) -> Node {
        self.w.nodes[0]
    }

    fn phase(&self) -> Phase {
        self.node().phase
    }

    fn credit(&self) -> i64 {
        self.node().credit.balance()
    }

    /// Confirm the node's effects one at a time until none is in flight.
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

    fn command(&mut self, command: Command) {
        self.cmd += 1;
        self.send(Event::Command {
            node: NodeId(0),
            id: self.cmd,
            command,
        });
        self.confirm();
    }

    fn answer(&mut self, answer: Answer) {
        let q = self.node().question.expect("an open question");
        self.command(Command::Answer {
            question: q.id,
            answer,
        });
    }

    fn asked_rob(&self) -> bool {
        self.notices.iter().any(|n| matches!(n, Notice::Ask { .. }))
    }

    fn plan(&mut self, files: impl IntoIterator<Item = FileId>) {
        let footprint = self.w.intern_footprint(files);
        self.reply(Reply::PlanLeaf {
            footprint,
            fits: true,
        });
    }

    fn needs_files(&mut self, files: impl IntoIterator<Item = FileId>) {
        let files = self.w.intern_footprint(files);
        self.reply(Reply::MakerNeedsFiles { files, fits: true });
    }

    fn start_making(&mut self) {
        self.reply(Reply::IngestDone { size: 1 });
        self.reply(Reply::AuditAgrees);
        self.plan([FileId(0)]);
        assert_eq!(self.phase(), Phase::Making);
    }

    /// One attempt whose first try and fix round both fail review.
    fn fail_attempt(&mut self, head: &mut HeadId) {
        for _ in 0..2 {
            head.0 += 1;
            self.reply(Reply::MakerDone { head: *head });
            self.reply(CHECKS_PASS);
            self.reply(Reply::ChangesRequested);
        }
    }

    fn pass(&mut self, head: &mut HeadId) {
        head.0 += 1;
        self.reply(Reply::MakerDone { head: *head });
        self.reply(CHECKS_PASS);
        self.reply(Reply::Approved { head: *head });
        assert_eq!(self.phase(), Phase::Queued);
    }
}

#[test]
fn a_clean_request_merges_without_asking_rob() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.fact(Fact::PrMerged {
        head,
        checks_passed: true,
    });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(run.credit(), 5); // 1 leaf + 3 reserve, then +1 for the merge
    assert!(!run.asked_rob());
}

/// The worked example from the design discussion: a one-leaf fix that keeps
/// failing review reaches Rob only when its credit runs out.
#[test]
fn a_stuck_fix_asks_rob_only_when_credit_runs_out() {
    let mut run = Run::new();
    run.start_making();
    assert_eq!(run.credit(), 4);
    let mut head = HeadId(0);

    run.fail_attempt(&mut head); // attempt 1 and its fix round are free; attempt 2 starts
    assert_eq!((run.node().attempt, run.credit()), (2, 3));
    run.fail_attempt(&mut head); // attempt 3 starts, on a different model
    assert_eq!((run.node().attempt, run.credit()), (3, 2));
    run.fail_attempt(&mut head); // one re-plan
    assert!(matches!(run.phase(), Phase::Planning { must_split: false }));
    assert_eq!(run.credit(), 1);

    run.plan([FileId(0)]);
    assert_eq!((run.node().attempt, run.credit()), (1, 1));
    run.fail_attempt(&mut head); // attempt 2 of the new plan
    assert_eq!((run.node().attempt, run.credit()), (2, 0));
    assert!(!run.asked_rob());

    run.fail_attempt(&mut head); // the next attempt needs 1 credit, and none is left
    assert_eq!(run.phase(), Phase::AwaitingRob);
    let q = run.node().question.unwrap();
    assert_eq!((q.kind, q.cost), (QKind::OutOfCredit, 1));

    run.answer(Answer::AddCredit(3));
    assert_eq!(
        (run.phase(), run.node().attempt, run.credit()),
        (Phase::Making, 3, 2)
    );
}

#[test]
fn every_slice_one_phase_is_reachable() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.command(Command::Pause);
    run.reply(Reply::AuditAgrees);
    assert_eq!(run.phase(), Phase::Unplanned); // paused, so planning waits
    run.command(Command::Resume);
    run.plan([FileId(0)]);
    run.command(Command::Pause);
    run.reply(Reply::MakerDone { head: HeadId(1) });
    assert_eq!(run.phase(), Phase::Ready(Resume::Checking)); // paused before the checks started
    assert!(!run.node().holds);
    run.command(Command::Resume);
    assert_eq!(run.phase(), Phase::Checking);
    run.reply(CHECKS_PASS);
    run.reply(Reply::Approved { head: HeadId(1) });
    run.fact(Fact::Conflict);
    assert_eq!(run.phase(), Phase::Rebasing);
    run.reply(Reply::RebaseClean { head: HeadId(2) });
    assert_eq!(run.phase(), Phase::Checking); // a new head is checked and reviewed again
    run.reply(CHECKS_PASS);
    run.reply(Reply::Approved { head: HeadId(2) });
    run.fact(Fact::PrMerged {
        head: HeadId(2),
        checks_passed: true,
    });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    let mut seen = run.seen.clone();

    let mut asked = Run::new();
    asked.reply(Reply::IngestDone { size: 1 });
    asked.reply(Reply::AuditDisagrees);
    asked.reply(Reply::IngestDone { size: 1 });
    asked.reply(Reply::AuditDisagrees);
    assert_eq!(asked.phase(), Phase::AwaitingRob);
    seen.extend(asked.seen.iter().copied());

    for phase in [
        "Ingesting",
        "Auditing",
        "Unplanned",
        "Planning",
        "Ready",
        "Making",
        "Checking",
        "Reviewing",
        "Publishing",
        "Queued",
        "Rebasing",
        "AwaitingRob",
        "Stopping",
        "Closed",
    ] {
        assert!(seen.contains(phase), "{phase} was never reached");
    }
}

#[test]
fn an_audit_disagreement_is_revised_once_before_rob_hears() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditDisagrees);
    assert_eq!(run.phase(), Phase::Ingesting);
    assert!(!run.asked_rob());
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditDisagrees);
    assert_eq!(run.node().question.unwrap().kind, QKind::AuditDisagreement);
    run.answer(Answer::Continue);
    assert!(matches!(run.phase(), Phase::Planning { .. }));
}

#[test]
fn a_queued_pr_can_still_merge_while_paused() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.command(Command::Pause);
    assert_eq!(run.phase(), Phase::Queued);
    run.fact(Fact::PrMerged {
        head,
        checks_passed: true,
    });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
}

#[test]
fn a_late_answer_is_ignored() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditDisagrees);
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditDisagrees);
    let question = run.node().question.unwrap().id;
    run.command(Command::Stop);
    assert!(run.phase().is_closed());
    let before = run.w.clone();
    run.command(Command::Answer {
        question,
        answer: Answer::Continue,
    });
    assert_eq!(run.w.nodes[0].phase, before.nodes[0].phase);
    assert_eq!(run.w.nodes[0].credit, before.nodes[0].credit);
}

#[test]
fn cleanup_that_fails_waits_for_rob_then_closes() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.send(Event::Fact {
        node: NodeId(0),
        fact: Fact::PrMerged {
            head,
            checks_passed: true,
        },
    });
    // Removing the worktree fails for good: cleanup holds on and asks Rob.
    let e = run.node().effect.expect("teardown in flight");
    run.send(Event::Ack {
        node: NodeId(0),
        seq: e.seq,
        ack: Ack::Permanent,
    });
    assert_eq!(run.phase(), Phase::Stopping(Outcome::Merged));
    assert_eq!(run.node().question.unwrap().kind, QKind::CleanupFailed);
    run.answer(Answer::MarkHandled);
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
}

#[test]
fn a_crash_loop_reaches_rob_after_its_second_incident() {
    let mut run = Run::new();
    run.start_making();
    run.reply(Reply::RunFailed);
    assert!(!run.node().waiting_incident);
    run.reply(Reply::RunFailed);
    assert!(run.node().waiting_incident && !run.node().incident_needs_rob);
    assert_eq!(run.phase(), Phase::Ready(Resume::Making)); // it gave back its slot
    run.fact(Fact::IncidentCleared { by_rob: false }); // a healthy probe clears the first
    assert_eq!(run.phase(), Phase::Making);
    run.reply(Reply::RunFailed);
    run.reply(Reply::RunFailed);
    assert!(run.node().incident_needs_rob);
    run.fact(Fact::IncidentCleared { by_rob: false }); // a probe can't clear the second
    assert!(run.node().waiting_incident);
    run.fact(Fact::IncidentCleared { by_rob: true });
    assert_eq!(run.phase(), Phase::Making);
}

#[test]
fn a_push_by_someone_else_is_checked_and_reviewed_again() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.fact(Fact::PushByOther { head: HeadId(7) });
    assert_eq!(run.phase(), Phase::Checking);
    assert_eq!(
        (run.node().head, run.node().checked),
        (Some(HeadId(7)), None)
    );
    assert!(!run.asked_rob());
}

#[test]
fn a_late_merge_after_a_stop_corrects_the_record() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.command(Command::Stop);
    assert_eq!(run.phase(), Phase::Closed(Outcome::Stopped));
    run.fact(Fact::PrMerged {
        head,
        checks_passed: true,
    });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(run.credit(), 5);
    run.fact(Fact::PrMerged {
        head,
        checks_passed: true,
    }); // credit applies once
    assert_eq!(run.credit(), 5);
}

#[test]
fn two_requests_share_one_slot_first_come_first_served() {
    let mut w = World::new(&[1], 2);
    for _ in 0..2 {
        w = step(w, Event::NewRequest { repo: RepoId(0) }).0;
    }
    // Drive both with the witness. The world must stay valid, the measure must
    // fall, and the two leaves must never hold the slot at once.
    for _ in 0..400 {
        let Some(e) = w.witness() else { break };
        let before = w.measure();
        let (next, _) = step(w, e);
        assert!(next.valid() && next.is_settled());
        assert!(next.measure() < before);
        assert!(next.nodes.iter().filter(|n| n.holds).count() <= 1);
        w = next;
    }
    assert!(
        w.nodes
            .iter()
            .all(|n| n.phase == Phase::Closed(Outcome::Merged))
    );
}

#[test]
fn the_one_replan_sends_one_note_and_keeps_working() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(1 << 40);
    for _ in 0..2 {
        run.fail_attempt(&mut head);
        assert!(!run.notices.contains(&Notice::Note(NoteKind::Replanned)));
    }
    run.fail_attempt(&mut head);
    assert_eq!(run.phase(), Phase::Planning { must_split: false });
    assert!(!run.node().holds);
    assert!(run.node().replanned);
    assert_eq!(run.credit(), 1);
    assert_eq!(
        run.notices
            .iter()
            .filter(|n| **n == Notice::Note(NoteKind::Replanned))
            .count(),
        1
    );
    assert!(!run.asked_rob());
    run.plan([FileId(1 << 40)]);
    run.pass(&mut head);
    run.fact(Fact::PrMerged {
        head,
        checks_passed: true,
    });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(
        run.notices
            .iter()
            .filter(|n| **n == Notice::Note(NoteKind::Replanned))
            .count(),
        1
    );
}

#[test]
fn a_replan_waiting_for_credit_sends_its_note_when_funded() {
    let mut run = Run::new();
    run.start_making();
    run.needs_files([FileId(90)]);
    run.needs_files([FileId(91)]);
    let mut head = HeadId(0);
    for _ in 0..3 {
        run.fail_attempt(&mut head);
    }
    assert_eq!(run.phase(), Phase::AwaitingRob);
    assert!(!run.notices.contains(&Notice::Note(NoteKind::Replanned)));
    run.answer(Answer::AddCredit(1));
    assert_eq!(run.phase(), Phase::Planning { must_split: false });
    assert_eq!(
        run.notices
            .iter()
            .filter(|n| **n == Notice::Note(NoteKind::Replanned))
            .count(),
        1
    );
}

#[test]
fn footprints_are_sets_with_more_than_eight_wide_file_ids() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditAgrees);
    let files = run
        .w
        .intern_footprint((0..300).rev().map(|i| FileId((1 << 40) + i)));
    run.reply(Reply::PlanLeaf {
        footprint: files,
        fits: true,
    });
    assert_eq!(run.node().footprint, files);
    run.needs_files([FileId(1 << 40), FileId(1 << 40), FileId(u64::MAX)]);
    let larger = run.node().footprint;
    assert_eq!(run.w.files(larger).unwrap().count(), 301);
    assert_eq!(run.w.files(larger).unwrap().last(), Some(FileId(u64::MAX)));
    assert!(
        run.w
            .files(files)
            .unwrap()
            .all(|f| run.w.files(larger).unwrap().any(|g| g == f))
    );
    assert!(!run.w.files(larger).unwrap().any(|f| f == FileId(7)));
    assert_eq!(run.credit(), 3);
}

#[test]
fn approval_is_one_exact_wide_head() {
    let mut run = Run::new();
    run.start_making();
    let mut head = HeadId(1 << 40);
    run.pass(&mut head);
    let old = head;
    assert_eq!(run.node().approved, Some(old));
    run.fact(Fact::Conflict);
    head = HeadId(u64::MAX);
    run.reply(Reply::RebaseClean { head });
    assert_eq!(run.phase(), Phase::Checking);
    run.reply(CHECKS_PASS);
    run.reply(Reply::Approved { head });
    assert_eq!(run.node().approved, Some(head));
    run.fact(Fact::PrMerged {
        head: old,
        checks_passed: true,
    });
    assert!(
        run.notices
            .contains(&Notice::Note(NoteKind::UnapprovedMerge))
    );
    assert_eq!(run.node().merged, Some(old));
}

#[test]
fn repo_and_node_ids_do_not_truncate() {
    let mut w = World::new(&vec![1; 300], 1);
    w = step(w, Event::NewRequest { repo: RepoId(299) }).0;
    assert_eq!(w.nodes[0].repo, RepoId(299));
    let before = w.clone();
    w = step(
        w,
        Event::Command {
            node: NodeId(1 << 32),
            id: 1,
            command: Command::Stop,
        },
    )
    .0;
    assert_eq!(w, before);
    w = step(
        w,
        Event::NewRequest {
            repo: RepoId(u64::MAX),
        },
    )
    .0;
    assert_eq!(w, before);
}

#[test]
fn file_locks_cover_large_footprints_and_stay_within_the_repo() {
    fn reply(w: &mut World, node: NodeId, reply: Reply) {
        let i = node.index().unwrap();
        while let Some(e) = w.nodes[i].effect {
            *w = step(
                w.clone(),
                Event::Ack {
                    node,
                    seq: e.seq,
                    ack: Ack::Confirmed,
                },
            )
            .0;
        }
        *w = step(
            w.clone(),
            Event::Reply {
                node,
                generation: w.nodes[i].generation,
                reply,
            },
        )
        .0;
        assert!(w.valid());
    }
    let mut w = World::new(&[2, 1], 3);
    for repo in [RepoId(0), RepoId(0), RepoId(1)] {
        w = step(w, Event::NewRequest { repo }).0;
    }
    let files = w.intern_footprint((0..300).map(|i| FileId((1 << 40) + i)));
    for node in [NodeId(0), NodeId(1), NodeId(2)] {
        reply(&mut w, node, Reply::IngestDone { size: 1 });
        reply(&mut w, node, Reply::AuditAgrees);
        reply(
            &mut w,
            node,
            Reply::PlanLeaf {
                footprint: files,
                fits: true,
            },
        );
    }
    assert_eq!(w.nodes[0].phase, Phase::Making);
    assert_eq!(w.nodes[1].phase, Phase::Ready(Resume::Making));
    assert_eq!(w.nodes[2].phase, Phase::Making);
    w = step(
        w,
        Event::Command {
            node: NodeId(0),
            id: 1,
            command: Command::Stop,
        },
    )
    .0;
    while let Some(e) = w.nodes[0].effect {
        w = step(
            w,
            Event::Ack {
                node: NodeId(0),
                seq: e.seq,
                ack: Ack::Confirmed,
            },
        )
        .0;
    }
    assert_eq!(w.nodes[1].phase, Phase::Making);
    assert!(w.nodes[1].holds && w.nodes[2].holds);
    assert!(w.valid());
}

#[test]
fn interned_footprints_keep_their_handles_across_growth_and_restore() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditAgrees);
    run.plan([FileId(0), FileId(63)]);
    let inline = run.node().footprint;
    run.needs_files([FileId(64), FileId(65), FileId(u64::MAX)]);
    let expanded = run.node().footprint;
    let expected = vec![
        FileId(0),
        FileId(63),
        FileId(64),
        FileId(65),
        FileId(u64::MAX),
    ];
    assert_eq!(run.w.files(expanded).unwrap().collect::<Vec<_>>(), expected);
    assert_eq!(
        run.w.files(inline).unwrap().collect::<Vec<_>>(),
        vec![FileId(0), FileId(63)]
    );
    let count = run.w.file_sets().len();
    assert_eq!(
        run.w.intern_footprint(expected.iter().copied().rev()),
        expanded
    );
    assert_eq!(run.w.file_sets().len(), count);
    let mut restored = World::new(&[1, 1], 1);
    for files in run.w.file_sets() {
        restored.intern_footprint(files.iter().copied());
    }
    assert_eq!(
        restored.files(expanded).unwrap().collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        restored.files(inline).unwrap().collect::<Vec<_>>(),
        vec![FileId(0), FileId(63)]
    );
}

#[test]
fn an_unknown_footprint_handle_is_a_failed_reply() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditAgrees);
    let mut other = World::new(&[1], 1);
    let foreign = other.intern_footprint([FileId(1000)]);
    assert!(run.w.files(foreign).is_none());
    run.reply(Reply::PlanLeaf {
        footprint: foreign,
        fits: true,
    });
    assert_eq!(run.phase(), Phase::Planning { must_split: false });
    assert_eq!(run.node().run_failures, 1);
}

#[test]
fn queue_stalls_raise_incidents_and_later_incidents_need_rob() {
    let mut run = Run::new();
    let before = run.node();
    run.fact(Fact::UpstreamStalled);
    assert_eq!(run.node(), before, "stalls outside Queued are ignored");
    run.start_making();
    let before = run.node();
    run.fact(Fact::UpstreamStalled);
    assert_eq!(run.node(), before);
    run.pass(&mut HeadId(0));
    run.fact(Fact::UpstreamStalled);
    assert!(run.node().waiting_incident && !run.node().incident_needs_rob);
    let before = run.node();
    let notices = run.notices.len();
    run.fact(Fact::UpstreamStalled);
    assert_eq!(
        run.node(),
        before,
        "an open incident absorbs repeated stalls"
    );
    assert_eq!(run.notices.len(), notices);
    run.fact(Fact::IncidentCleared { by_rob: false });
    assert!(!run.node().waiting_incident);
    run.fact(Fact::UpstreamStalled);
    assert!(run.node().waiting_incident && run.node().incident_needs_rob);
    run.fact(Fact::IncidentCleared { by_rob: false });
    assert!(run.node().waiting_incident);
    run.fact(Fact::IncidentCleared { by_rob: true });
    assert!(!run.node().waiting_incident);
}

fn q44_review() -> Run {
    let mut run = Run::new();
    run.start_making();
    run.reply(Reply::MakerDone { head: HeadId(1) });
    run.reply(CHECKS_PASS);
    assert_eq!(run.phase(), Phase::Reviewing);
    run
}

#[test]
fn q44_first_exhaustion_starts_a_free_fresh_review() {
    let mut run = q44_review();
    let before = run.node();
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::Reviewing);
    assert_eq!(run.node().run_failures, 0, "exhaustion is not a crash");
    assert_eq!(run.node().rechecks, 1);
    assert_eq!(run.node().credit, before.credit);
    assert!(run.node().generation > before.generation);
    assert_eq!(run.node().checked, before.checked);
    assert!(run.node().holds);
    run.reply(Reply::Approved { head: HeadId(1) });
    assert_eq!(run.phase(), Phase::Queued);
}

#[test]
fn q44_fresh_review_shares_the_rebase_recheck_allowance() {
    for credit in [0, 1] {
        let mut run = q44_review();
        run.w.nodes[0].rechecks = FREE_RECHECKS;
        run.w.nodes[0].credit.spent = run.w.nodes[0].credit.start - credit;
        run.reply(Reply::ReviewerOutOfRoom);
        assert_eq!(run.node().rechecks, FREE_RECHECKS + 1);
        if credit == 0 {
            assert_eq!(run.phase(), Phase::AwaitingRob);
            let question = run.node().question.unwrap();
            assert_eq!(question.kind, QKind::OutOfCredit);
            assert_eq!(question.cont, Cont::Ready(Resume::Reviewing));
            assert!(!run.node().holds);
            run.answer(Answer::AddCredit(1));
        }
        assert_eq!(run.phase(), Phase::Reviewing);
        assert_eq!(run.credit(), 0);
        run.reply(Reply::Approved { head: HeadId(1) });
        assert_eq!(run.phase(), Phase::Queued);
    }
}

#[test]
fn q44_second_exhaustion_requires_a_split() {
    let mut run = q44_review();
    run.reply(Reply::ReviewerOutOfRoom);
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::Planning { must_split: true });
    assert_eq!(run.credit(), 3);
    assert!(!run.node().holds);
    run.plan([FileId(0)]);
    assert_eq!(run.phase(), Phase::Planning { must_split: true });
    run.reply(Reply::PlanSplit);
    assert_eq!(run.node().question.unwrap().kind, QKind::NeedsSplitting);
}

#[test]
fn q44_second_exhaustion_without_credit_keeps_the_split_continuation() {
    let mut run = q44_review();
    run.w.nodes[0].credit.spent = run.w.nodes[0].credit.start;
    run.reply(Reply::ReviewerOutOfRoom);
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::AwaitingRob);
    let question = run.node().question.unwrap();
    assert_eq!(question.kind, QKind::OutOfCredit);
    assert_eq!(question.cont, Cont::Planning { must_split: true });
    assert!(!run.node().holds);
    run.answer(Answer::AddCredit(1));
    assert_eq!(run.phase(), Phase::Planning { must_split: true });
    assert_eq!(run.credit(), 0);
}

#[test]
fn q44_fix_round_keeps_the_fresh_reviewer_and_a_new_attempt_resets_it() {
    let mut run = q44_review();
    run.reply(Reply::ReviewerOutOfRoom);
    run.reply(Reply::ChangesRequested);
    run.reply(Reply::MakerDone { head: HeadId(2) });
    run.reply(CHECKS_PASS);
    assert!(run.node().review_restarted);
    run.reply(Reply::ChangesRequested);
    assert_eq!(run.node().attempt, 2);
    assert!(!run.node().review_restarted);
    run.reply(Reply::MakerDone { head: HeadId(3) });
    run.reply(CHECKS_PASS);
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::Reviewing);
    assert!(run.node().review_restarted);
    assert_eq!(run.node().rechecks, 2);
}

fn q44_new_head_review(rebase: bool) {
    let mut run = q44_review();
    run.reply(Reply::ReviewerOutOfRoom);
    run.reply(Reply::Approved { head: HeadId(1) });
    assert_eq!(run.phase(), Phase::Queued);
    if rebase {
        run.fact(Fact::Conflict);
        run.reply(Reply::RebaseClean { head: HeadId(2) });
    } else {
        run.fact(Fact::PushByOther { head: HeadId(2) });
    }
    assert!(
        !run.node().review_restarted,
        "a new head gets its own fresh review allowance"
    );
    run.reply(CHECKS_PASS);
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::Reviewing);
    assert_eq!(run.node().head, Some(HeadId(2)));
    assert_eq!(
        run.node().rechecks,
        3,
        "new reviews retain the shared recheck counter"
    );
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.phase(), Phase::Planning { must_split: true });
}

#[test]
fn q44_clean_rebase_starts_a_new_review_allowance() {
    q44_new_head_review(true);
}

#[test]
fn q44_outside_push_starts_a_new_review_allowance() {
    q44_new_head_review(false);
}

#[test]
fn q44_the_same_head_keeps_its_exhaustion_history() {
    for rebase in [false, true] {
        let mut run = q44_review();
        run.reply(Reply::ReviewerOutOfRoom);
        run.reply(Reply::Approved { head: HeadId(1) });
        if rebase {
            run.fact(Fact::Conflict);
            run.reply(Reply::RebaseClean { head: HeadId(1) });
        } else {
            run.fact(Fact::PushByOther { head: HeadId(1) });
        }
        assert!(run.node().review_restarted);
        run.reply(CHECKS_PASS);
        run.reply(Reply::ReviewerOutOfRoom);
        assert_eq!(run.phase(), Phase::Planning { must_split: true });
    }
}

fn adoption_run() -> Run {
    let mut run = Run {
        w: World::new(&[1], 1),
        cmd: 0,
        seen: BTreeSet::new(),
        notices: Vec::new(),
    };
    run.send(Event::NewAdoption {
        repo: RepoId(0),
        head: HeadId(7),
    });
    run.confirm();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditAgrees);
    run.plan([FileId(0)]);
    run
}

#[test]
fn adoption_enters_checks_without_a_maker_or_credit_charge() {
    let mut run = adoption_run();
    assert_eq!(run.phase(), Phase::Checking);
    assert_eq!(run.node().head, Some(HeadId(7)));
    assert_eq!(
        (run.node().attempt, run.node().round, run.credit()),
        (1, 0, 4)
    );
    assert!(run.node().adopted);
    run.reply(CHECKS_PASS);
    run.reply(Reply::Approved { head: HeadId(7) });
    run.fact(Fact::PrMerged {
        head: HeadId(7),
        checks_passed: true,
    });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(run.credit(), 5);
}

#[test]
fn adoption_failure_runs_a_normal_fix_and_never_readopts() {
    let mut run = adoption_run();
    run.reply(Reply::ChecksDone {
        report: true,
        pass: false,
        in_footprint: true,
    });
    assert_eq!(run.phase(), Phase::Making);
    assert_eq!(
        (run.node().attempt, run.node().round, run.credit()),
        (1, 1, 4)
    );
    assert_eq!(run.node().head, Some(HeadId(7)));
    let mut head = HeadId(7);
    run.pass(&mut head);
    assert_eq!(run.node().head, Some(HeadId(8)));
    run.fact(Fact::CiFailed { infra: false });
    assert_eq!(run.phase(), Phase::Making);
    assert_eq!(run.node().attempt, 2);
    assert_eq!(run.node().head, Some(HeadId(8)));
    assert_eq!(run.credit(), 3);
}

#[test]
fn publishing_conflict_rebases_at_every_publication_step() {
    for confirmed in 0..4 {
        let mut r = Run::new();
        r.start_making();
        r.reply(Reply::MakerDone { head: HeadId(1) });
        r.reply(CHECKS_PASS);
        r.send(Event::Reply {
            node: NodeId(0),
            generation: r.node().generation,
            reply: Reply::Approved { head: HeadId(1) },
        });
        for _ in 0..confirmed {
            let effect = r.node().effect.unwrap();
            r.send(Event::Ack {
                node: NodeId(0),
                seq: effect.seq,
                ack: Ack::Confirmed,
            });
        }
        let generation = r.node().generation;
        let credit = r.credit();
        r.fact(Fact::Conflict);
        assert_eq!(r.phase(), Phase::Rebasing);
        assert!(r.node().generation > generation);
        assert_eq!(r.credit(), credit);
        let generation = r.node().generation;
        r.fact(Fact::Conflict);
        assert_eq!(
            r.node().generation,
            generation,
            "duplicate conflict must not restart rebasing"
        );
        r.reply(Reply::RebaseNeedsChanges);
        assert_eq!(r.phase(), Phase::Making);
        assert_eq!(r.credit(), credit - 1);
        let mut head = HeadId(1);
        r.pass(&mut head);
        r.fact(Fact::PrMerged {
            head,
            checks_passed: true,
        });
        assert_eq!(r.phase(), Phase::Closed(Outcome::Merged));
        assert!(!r.asked_rob());
    }
}

#[test]
fn publishing_conflict_cycles_exhaust_credit_or_recheck_allowance() {
    for clean in [false, true] {
        let mut r = Run::new();
        r.start_making();
        let mut head = HeadId(1);
        r.reply(Reply::MakerDone { head });
        r.reply(CHECKS_PASS);
        loop {
            r.send(Event::Reply {
                node: NodeId(0),
                generation: r.node().generation,
                reply: Reply::Approved { head },
            });
            r.fact(Fact::Conflict);
            assert_eq!(r.phase(), Phase::Rebasing);
            head.0 += 1;
            r.reply(if clean {
                Reply::RebaseClean { head }
            } else {
                Reply::RebaseNeedsChanges
            });
            if r.phase() == Phase::AwaitingRob {
                break;
            }
            if !clean {
                r.reply(Reply::MakerDone { head });
            }
            r.reply(CHECKS_PASS);
            assert!(
                head.0 <= u64::from(FREE_RECHECKS) + 6,
                "conflicts cannot loop for free"
            );
        }
        assert_eq!(r.credit(), 0);
        assert!(!r.node().holds);
        assert_eq!(r.node().question.unwrap().kind, QKind::OutOfCredit);
    }
}

#[test]
fn settle_world_grants_reconciled_capacity_and_is_idempotent() {
    fn confirm(mut world: World) -> World {
        for _ in 0..64 {
            let Some((i, effect)) = world
                .nodes
                .iter()
                .enumerate()
                .find_map(|(i, n)| n.effect.map(|e| (i, e)))
            else {
                return world;
            };
            world = step(
                world,
                Event::Ack {
                    node: NodeId(i as u64),
                    seq: effect.seq,
                    ack: Ack::Confirmed,
                },
            )
            .0;
        }
        panic!("effects did not settle");
    }
    let mut world = World::new(&[1], 3);
    for _ in 0..3 {
        world = confirm(step(world, Event::NewRequest { repo: RepoId(0) }).0);
    }
    for i in 0..3 {
        for reply in [Reply::IngestDone { size: 1 }, Reply::AuditAgrees] {
            let generation = world.nodes[i].generation;
            world = confirm(
                step(
                    world,
                    Event::Reply {
                        node: NodeId(i as u64),
                        generation,
                        reply,
                    },
                )
                .0,
            );
        }
    }
    for i in 0..3 {
        let generation = world.nodes[i].generation;
        let footprint = world.intern_footprint([FileId(i as u64)]);
        world = confirm(
            step(
                world,
                Event::Reply {
                    node: NodeId(i as u64),
                    generation,
                    reply: Reply::PlanLeaf {
                        footprint,
                        fits: true,
                    },
                },
            )
            .0,
        );
    }
    assert_eq!(world.nodes.iter().filter(|n| n.holds).count(), 1);
    let before = world.clone();
    world.gates[0].w = 3;
    let (world, out) = settle_world(world);
    assert!(world.valid() && world.is_settled());
    assert!(
        world
            .nodes
            .iter()
            .all(|n| n.holds && n.phase == Phase::Making)
    );
    assert_eq!(world.nodes[0], before.nodes[0]);
    assert_eq!(out.len(), 2);
    for node in [NodeId(1), NodeId(2)] {
        assert!(
            out.iter()
                .any(|o| matches!(o, Out::Effect { node: target, .. } if *target == node))
        );
    }
    let (again, out) = settle_world(world.clone());
    assert_eq!(again, world);
    assert!(out.is_empty(), "settle must not repeat grants or effects");
}

fn publishing_enqueue() -> Run {
    let mut run = Run::new();
    run.start_making();
    run.pass(&mut HeadId(0));
    let node = &mut run.w.nodes[0];
    node.phase = Phase::Publishing;
    node.pos = 3;
    node.effect = Some(InFlight {
        seq: node.next_seq,
        kind: EffectKind::Enqueue,
        cancelling: false,
    });
    node.next_seq += 1;
    assert!(run.w.valid() && run.w.is_settled());
    run
}

#[test]
fn publishing_ci_test_failure_cancels_enqueue_and_starts_fix_round() {
    let mut run = publishing_enqueue();
    let credit = run.credit();
    let enqueue = run.node().effect.unwrap();
    run.send(Event::Fact {
        node: NodeId(0),
        fact: Fact::CiFailed { infra: false },
    });
    assert_eq!(run.phase(), Phase::Making);
    assert_eq!(run.node().round, 1);
    assert_eq!(run.credit(), credit);
    assert!(run.node().effect_cancelling(enqueue.seq));
    run.confirm();
    assert_eq!(run.phase(), Phase::Making);
}

#[test]
fn publishing_ci_failed_fix_round_respects_credit() {
    let mut run = publishing_enqueue();
    run.w.nodes[0].round = 1;
    run.w.nodes[0].credit.spent = run.w.nodes[0].credit.start;
    let enqueue = run.node().effect.unwrap();
    run.send(Event::Fact {
        node: NodeId(0),
        fact: Fact::CiFailed { infra: false },
    });
    assert_eq!(run.phase(), Phase::AwaitingRob);
    assert_eq!(run.credit(), 0);
    assert!(run.node().effect_cancelling(enqueue.seq));
}

#[test]
fn publishing_ci_infrastructure_retries_once_then_raises_incident() {
    for failures in [0, 1] {
        let mut run = publishing_enqueue();
        run.w.nodes[0].run_failures = failures;
        let credit = run.credit();
        let enqueue = run.node().effect.unwrap();
        run.send(Event::Fact {
            node: NodeId(0),
            fact: Fact::CiFailed { infra: true },
        });
        assert!(run.node().effect_cancelling(enqueue.seq));
        assert_eq!(run.credit(), credit);
        assert_eq!(run.node().waiting_incident, failures == 1);
        assert_eq!(run.node().run_failures, 1 - failures);
        run.confirm();
        assert_eq!(run.phase(), Phase::Publishing);
    }
}

#[test]
fn publishing_ci_failure_before_enqueue_step_is_ignored() {
    let mut run = publishing_enqueue();
    run.w.nodes[0].pos = 1;
    run.w.nodes[0].effect.as_mut().unwrap().kind = EffectKind::PostApproval;
    let before = run.node();
    run.send(Event::Fact {
        node: NodeId(0),
        fact: Fact::CiFailed { infra: false },
    });
    assert_eq!(run.node(), before);
}
