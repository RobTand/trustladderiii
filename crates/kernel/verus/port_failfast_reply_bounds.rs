//! Q54 / #832-A / #806: per-branch actor bounds for the six fail-fast replies.
//! Each lemma opens only the handler relations of one reply branch, so the
//! headroom and flag-off review-stop compositions never unfold every row at once.
use super::*;

verus! {
/// #806: one reply's counter growth stays within the complete checker's step budgets.
pub(super) open spec fn grown(b: Node, n: Node) -> bool {
    n.generation as int <= b.generation as int + 2
        && n.next_seq as int <= b.next_seq as int + 1
        && n.credit.start as int <= b.credit.start as int + CREDIT_START_STEP_BUDGET
        && n.credit.earned as int <= b.credit.earned as int + 1
        && n.credit.added as int <= b.credit.added as int + CREDIT_ADDED_STEP_BUDGET
        && n.credit.spent as int <= b.credit.spent as int + CREDIT_SPENT_STEP_BUDGET
}

/// Q54 / #832-A / #806: actor counters and question IDs grow within budget, and a
/// flag-off reply never changes review_stop.
pub(super) open spec fn bound(before: World, after: World) -> bool {
    grown(before.nodes@[0], after.nodes@[0])
        && after.next_question as int <= before.next_question as int + 1
        && (!before.fail_fast ==> after.nodes@[0].review_stop == before.nodes@[0].review_stop)
}

/// Q30 stopping, stale and schema-rejected rows: release, stutter, or the failed-run restart.
pub(super) proof fn inactive(before: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires before.nodes@.len() == 2,
        !port_review_reply::active(before.nodes@[0], generation)
            || !port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, initial, out),
    ensures bound(before, after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_retry::failed_run); reveal(retirement::incident);
}

/// Maker ladder and exhaustion rows / Q54: MakerCantMeet and MakerOutOfRoom.
pub(super) proof fn maker_work(before: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(before), port_failfast_replies::selected(before.nodes@[0], reply),
        matches!(reply, Reply::MakerCantMeet | Reply::MakerOutOfRoom),
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, initial, out),
    ensures bound(before, after),
{
    port_failfast_replies::admitted(before, after, generation, reply, initial, out);
    reveal(port_review_reply::accepted_reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::work_result);
    reveal(port_review_retry::rung); reveal(port_review_retry::room); reveal(retirement::stopped);
}

/// File extension row / Q54 funding: funded union, unfunded question, or room retry.
pub(super) proof fn maker_files(before: World, after: World, generation: u64, files: Footprint,
    fits: bool, initial: Outbox, out: Outbox)
    requires retirement::domain(before), before.nodes@[0].phase == Phase::Making,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], Reply::MakerNeedsFiles { files, fits }),
        port_review_reply::reply_post(before, after, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, out),
    ensures bound(before, after),
{
    port_failfast_replies::admitted(before, after, generation,
        Reply::MakerNeedsFiles { files, fits }, initial, out);
    reveal(port_review_reply::accepted_reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::files_post);
    reveal(port_review_retry::room); reveal(retirement::stopped);
}

/// Review and checks fix-round rows / Q54: ChangesRequested and ChecksDone.
pub(super) proof fn review_round(before: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(before), port_failfast_replies::selected(before.nodes@[0], reply),
        matches!(reply, Reply::ChangesRequested | Reply::ChecksDone { .. }),
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, initial, out),
    ensures bound(before, after),
{
    port_failfast_replies::admitted(before, after, generation, reply, initial, out);
    reveal(port_review_reply::transition);
    reveal(port_review_retry::round); reveal(port_review_retry::rung); reveal(retirement::stopped);
}

/// Reviewer exhaustion row / Q54: first restart rechecks, later exhaustion retries.
pub(super) proof fn review_room(before: World, after: World, generation: u64,
    initial: Outbox, out: Outbox)
    requires retirement::domain(before), before.nodes@[0].phase == Phase::Reviewing,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], Reply::ReviewerOutOfRoom),
        port_review_reply::reply_post(before, after, 0, generation, Reply::ReviewerOutOfRoom, initial, out),
    ensures bound(before, after),
{
    port_failfast_replies::admitted(before, after, generation, Reply::ReviewerOutOfRoom, initial, out);
    reveal(port_review_reply::transition);
    reveal(port_review_retry::room); reveal(port_review_retry::rechecked); reveal(retirement::stopped);
}

/// Q54 / #806: dispatch every selected reply and generation to its branch bound.
pub(super) proof fn actor(before: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(before), port_failfast_replies::selected(before.nodes@[0], reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, initial, out),
    ensures bound(before, after),
{
    if !port_review_reply::active(before.nodes@[0], generation)
        || !port_review_reply::accepted_reply(before, before.nodes@[0], reply) {
        inactive(before, after, generation, reply, initial, out);
    } else if let Reply::MakerNeedsFiles { files, fits } = reply {
        maker_files(before, after, generation, files, fits, initial, out);
    } else if matches!(reply, Reply::MakerCantMeet | Reply::MakerOutOfRoom) {
        maker_work(before, after, generation, reply, initial, out);
    } else if reply == Reply::ReviewerOutOfRoom {
        review_room(before, after, generation, initial, out);
    } else {
        review_round(before, after, generation, reply, initial, out);
    }
}
}
