//! #806: real apply, complete checker and answer assertions for all eight answers.
use super::*;
use super::port_answers_model::*;

verus! {
/// #806: carry an unchanged node through the one monotone question allocation.
pub(super) proof fn unchanged_node(before: World, after: World, n: Node)
    requires retirement::node_rules(before, n), after.gates@ == before.gates@,
        after.file_sets@ == before.file_sets@, after.next_ticket == before.next_ticket,
        after.next_question >= before.next_question,
    ensures retirement::node_rules(after, n),
{
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// #806: compose per-answer frames using the approved two-node checker domain.
pub(super) proof fn preserves_world(before: World, after: World, cmd: u64, id: u64, answer: Answer)
    requires retirement::domain(before), retirement::accepted(before),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX,
        after.nodes@ == before.nodes@.update(0, commanded(before.nodes@[0], cmd, id, answer, before.next_question)),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
        after.next_question == next_question(before.nodes@[0], cmd, id, answer, before.next_question),
        after.pool_capacity == before.pool_capacity, after.dependencies@ == before.dependencies@,
    ensures retirement::domain(after), retirement::accepted(after),
        shared(before.nodes@[0], after.nodes@[0], id),
        shared(before.nodes@[1], after.nodes@[1], id),
{
    reveal(retirement::accepted);
    let n = before.nodes@[0];
    match answer {
        Answer::Continue => port_answers_frames::continue_frame(before, after, n, cmd, id),
        Answer::TryAgain => port_answers_frames::try_again_frame(before, after, n, cmd, id),
        Answer::AddCredit(k) => port_answers_frames::add_credit_frame(before, after, n, cmd, id, k),
        Answer::ChangeRequest => port_answers_frames::change_request_frame(before, after, n, cmd, id),
        Answer::Stop => port_answers_frames::stop_frame(before, after, n, cmd, id),
        Answer::Unclear => port_answers_frames::unclear_frame(before, after, n, cmd, id),
        Answer::Retry => port_answers_frames::retry_frame(before, after, n, cmd, id),
        Answer::MarkHandled => port_answers_frames::mark_handled_frame(before, after, n, cmd, id),
    }
    unchanged_node(before, after, before.nodes@[1]);
    assert forall|r: int| 0 <= r < 2 implies
        retirement::holders(after, RepoId(r as u64), 2) <= after.gates@[r].w by {
        assert(retirement::holders(after, RepoId(r as u64), 2)
            <= retirement::holders(before, RepoId(r as u64), 2));
    }
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}

kernel_fn! {
/// #806: all eight check_apply(answer(...)) harnesses, with arbitrary command and
/// question IDs and arbitrary u8 credit. One real apply, no settle or unrolling.
pub(super) fn verify_answer(mut w: World, cmd: u64, id: u64, answer: Answer) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
    ensures retirement::domain(result.0), retirement::accepted(result.0),
        result.0.nodes@ == w.nodes@.update(0, commanded(w.nodes@[0], cmd, id, answer, w.next_question)),
        result.0.next_question == next_question(w.nodes@[0], cmd, id, answer, w.next_question),
        retirement::notices_only(result.1),
        shared(w.nodes@[0], result.0.nodes@[0], id), shared(w.nodes@[1], result.0.nodes@[1], id),
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(storage_ordered);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(retirement::accepted); reveal(retirement::bounded); reveal(retirement::notices_only);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        assert(world_headroom(w));
        dependencies::indexable_definition(w);
    }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    let mut out = Outbox::new();
    apply(&mut w, Event::Command { node: NodeId(0), id: cmd,
        command: Command::Answer { question: id, answer } }, &mut out);
    kernel_proof! { preserves_world(before@, w, cmd, id, answer); }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out)
}
}
