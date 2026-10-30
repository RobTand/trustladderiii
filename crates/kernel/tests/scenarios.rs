//! Scripted runs through the slice-one kernel. Every step checks that the
//! world stays valid and settled.

use std::collections::BTreeSet;
use tl3_kernel::*;

const CHECKS_PASS: Reply = Reply::ChecksDone { report: true, pass: true, in_footprint: true };

struct Run {
    w: World,
    cmd: u64,
    seen: BTreeSet<&'static str>,
    notices: Vec<Notice>,
}

impl Run {
    fn new() -> Run {
        let mut run =
            Run { w: World::new(&[1, 1], 1), cmd: 0, seen: BTreeSet::new(), notices: Vec::new() };
        run.send(Event::NewRequest { repo: RepoId(0) });
        run.confirm();
        run
    }

    fn send(&mut self, event: Event) {
        let (w, out) = step(self.w.clone(), event.clone());
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
        self.w.nodes[0].clone()
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
            self.send(Event::Ack { node: NodeId(0), seq: e.seq, ack: Ack::Confirmed });
        }
        panic!("effects never stopped");
    }

    fn reply(&mut self, reply: Reply) {
        self.confirm();
        let generation = self.node().generation;
        self.send(Event::Reply { node: NodeId(0), generation, reply });
        self.confirm();
    }

    fn fact(&mut self, fact: Fact) {
        self.send(Event::Fact { node: NodeId(0), fact });
        self.confirm();
    }

    fn command(&mut self, command: Command) {
        self.cmd += 1;
        self.send(Event::Command { node: NodeId(0), id: self.cmd, command });
        self.confirm();
    }

    fn answer(&mut self, answer: Answer) {
        let q = self.node().question.expect("an open question");
        self.command(Command::Answer { question: q.id, answer });
    }

    fn asked_rob(&self) -> bool {
        self.notices.iter().any(|n| matches!(n, Notice::Ask { .. }))
    }

    fn to_making(&mut self) {
        self.reply(Reply::IngestDone { size: 1 });
        self.reply(Reply::AuditAgrees);
        self.reply(Reply::PlanLeaf { footprint: Footprint::new([FileId(0)]), fits: true });
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
    run.to_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.fact(Fact::PrMerged { head, checks_passed: true });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(run.credit(), 5); // 1 leaf + 3 reserve, then +1 for the merge
    assert!(!run.asked_rob());
}

/// The worked example from the design discussion: a one-leaf fix that keeps
/// failing review reaches Rob only when its credit runs out.
#[test]
fn a_stuck_fix_asks_rob_only_when_credit_runs_out() {
    let mut run = Run::new();
    run.to_making();
    assert_eq!(run.credit(), 4);
    let mut head = HeadId(0);

    run.fail_attempt(&mut head); // attempt 1 and its fix round are free; attempt 2 starts
    assert_eq!((run.node().attempt, run.credit()), (2, 3));
    run.fail_attempt(&mut head); // attempt 3 starts, on a different model
    assert_eq!((run.node().attempt, run.credit()), (3, 2));
    run.fail_attempt(&mut head); // one re-plan
    assert!(matches!(run.phase(), Phase::Planning { must_split: false }));
    assert_eq!(run.credit(), 1);

    run.reply(Reply::PlanLeaf { footprint: Footprint::new([FileId(0)]), fits: true });
    assert_eq!((run.node().attempt, run.credit()), (1, 1));
    run.fail_attempt(&mut head); // attempt 2 of the new plan
    assert_eq!((run.node().attempt, run.credit()), (2, 0));
    assert!(!run.asked_rob());

    run.fail_attempt(&mut head); // the next attempt needs 1 credit, and none is left
    assert_eq!(run.phase(), Phase::AwaitingRob);
    let q = run.node().question.unwrap();
    assert_eq!((q.kind, q.cost), (QKind::OutOfCredit, 1));

    run.answer(Answer::AddCredit(3));
    assert_eq!((run.phase(), run.node().attempt, run.credit()), (Phase::Making, 3, 2));
}

#[test]
fn every_slice_one_phase_is_reachable() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.command(Command::Pause);
    run.reply(Reply::AuditAgrees);
    assert_eq!(run.phase(), Phase::Unplanned); // paused, so planning waits
    run.command(Command::Resume);
    run.reply(Reply::PlanLeaf { footprint: Footprint::new([FileId(0)]), fits: true });
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
    run.fact(Fact::PrMerged { head: HeadId(2), checks_passed: true });
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
        "Ingesting", "Auditing", "Unplanned", "Planning", "Ready", "Making", "Checking",
        "Reviewing", "Publishing", "Queued", "Rebasing", "AwaitingRob", "Stopping", "Closed",
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
    run.to_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.command(Command::Pause);
    assert_eq!(run.phase(), Phase::Queued);
    run.fact(Fact::PrMerged { head, checks_passed: true });
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
    run.command(Command::Answer { question, answer: Answer::Continue });
    assert_eq!(run.w.nodes[0].phase, before.nodes[0].phase);
    assert_eq!(run.w.nodes[0].credit, before.nodes[0].credit);
}

#[test]
fn cleanup_that_fails_waits_for_rob_then_closes() {
    let mut run = Run::new();
    run.to_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.send(Event::Fact { node: NodeId(0), fact: Fact::PrMerged { head, checks_passed: true } });
    // Removing the worktree fails for good: cleanup holds on and asks Rob.
    let e = run.node().effect.expect("teardown in flight");
    run.send(Event::Ack { node: NodeId(0), seq: e.seq, ack: Ack::Permanent });
    assert_eq!(run.phase(), Phase::Stopping(Outcome::Merged));
    assert_eq!(run.node().question.unwrap().kind, QKind::CleanupFailed);
    run.answer(Answer::MarkHandled);
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
}

#[test]
fn a_crash_loop_reaches_rob_after_its_second_incident() {
    let mut run = Run::new();
    run.to_making();
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
    run.to_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.fact(Fact::PushByOther { head: HeadId(7) });
    assert_eq!(run.phase(), Phase::Checking);
    assert_eq!((run.node().head, run.node().checked), (Some(HeadId(7)), None));
    assert!(!run.asked_rob());
}

#[test]
fn a_late_merge_after_a_stop_corrects_the_record() {
    let mut run = Run::new();
    run.to_making();
    let mut head = HeadId(0);
    run.pass(&mut head);
    run.command(Command::Stop);
    assert_eq!(run.phase(), Phase::Closed(Outcome::Stopped));
    run.fact(Fact::PrMerged { head, checks_passed: true });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(run.credit(), 5);
    run.fact(Fact::PrMerged { head, checks_passed: true }); // credit applies once
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
    assert!(w.nodes.iter().all(|n| n.phase == Phase::Closed(Outcome::Merged)));
}

#[test]
fn the_one_replan_sends_one_note_and_keeps_working() {
    let mut run = Run::new();
    run.to_making();
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
    assert_eq!(run.notices.iter().filter(|n| **n == Notice::Note(NoteKind::Replanned)).count(), 1);
    assert!(!run.asked_rob());
    run.reply(Reply::PlanLeaf { footprint: Footprint::new([FileId(1 << 40)]), fits: true });
    run.pass(&mut head);
    run.fact(Fact::PrMerged { head, checks_passed: true });
    assert_eq!(run.phase(), Phase::Closed(Outcome::Merged));
    assert_eq!(run.notices.iter().filter(|n| **n == Notice::Note(NoteKind::Replanned)).count(), 1);
}

#[test]
fn a_replan_waiting_for_credit_sends_its_note_when_funded() {
    let mut run = Run::new();
    run.to_making();
    run.reply(Reply::MakerNeedsFiles { files: Footprint::new([FileId(90)]), fits: true });
    run.reply(Reply::MakerNeedsFiles { files: Footprint::new([FileId(91)]), fits: true });
    let mut head = HeadId(0);
    for _ in 0..3 {
        run.fail_attempt(&mut head);
    }
    assert_eq!(run.phase(), Phase::AwaitingRob);
    assert!(!run.notices.contains(&Notice::Note(NoteKind::Replanned)));
    run.answer(Answer::AddCredit(1));
    assert_eq!(run.phase(), Phase::Planning { must_split: false });
    assert_eq!(run.notices.iter().filter(|n| **n == Notice::Note(NoteKind::Replanned)).count(), 1);
}

#[test]
fn footprints_are_sets_with_more_than_eight_wide_file_ids() {
    let mut run = Run::new();
    run.reply(Reply::IngestDone { size: 1 });
    run.reply(Reply::AuditAgrees);
    let files = Footprint::new((0..300).rev().map(|i| FileId((1 << 40) + i)));
    run.reply(Reply::PlanLeaf { footprint: files.clone(), fits: true });
    assert_eq!(run.node().footprint, files);
    run.reply(Reply::MakerNeedsFiles {
        files: Footprint::new([FileId(1 << 40), FileId(1 << 40), FileId(u64::MAX)]), fits: true,
    });
    let larger = run.node().footprint;
    assert_eq!(larger.files().len(), 301);
    assert_eq!(larger.files().last(), Some(&FileId(u64::MAX)));
    assert!(!larger.is_disjoint(&files));
    assert!(larger.is_disjoint(&Footprint::new([FileId(7)])));
    assert_eq!(run.credit(), 3);
}

#[test]
fn approval_is_one_exact_wide_head() {
    let mut run = Run::new();
    run.to_making();
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
    run.fact(Fact::PrMerged { head: old, checks_passed: true });
    assert!(run.notices.contains(&Notice::Note(NoteKind::UnapprovedMerge)));
    assert_eq!(run.node().merged, Some(old));
}

#[test]
fn repo_and_node_ids_do_not_truncate() {
    let mut w = World::new(&vec![1; 300], 1);
    w = step(w, Event::NewRequest { repo: RepoId(299) }).0;
    assert_eq!(w.nodes[0].repo, RepoId(299));
    let before = w.clone();
    w = step(w, Event::Command { node: NodeId(1 << 32), id: 1, command: Command::Stop }).0;
    assert_eq!(w, before);
    w = step(w, Event::NewRequest { repo: RepoId(u64::MAX) }).0;
    assert_eq!(w, before);
}
