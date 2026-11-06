//! #806: constructive, original-domain changed-world covers for the eight answers.
use super::port_answers_model::*;
use super::*;

kernel_fn! {
/// #806: two nodes, two repositories, empty storage and dependencies, small
/// counters and credit, complete consistency and settledness as in any_settled_world.
fn world(case: u8) -> (result: World) [
    requires case < 8,
    ensures progress_phase_witness::original(result), retirement::accepted(result), settled_world(result), storage_ordered(result),
        result.next_question == 1, result.nodes@[0].last_cmd == 0,
        result.nodes@[0].question.is_some(), result.nodes@[0].question.unwrap().id == 0,
        !result.nodes@[0].question.unwrap().clarified,
        result.nodes@[0].phase == if case < 6 { Phase::AwaitingRob } else { Phase::Stopping(Outcome::Stopped) },
        result.nodes@[0].credit.added == 0, result.nodes@[0].credit.spent == 0,
        result.nodes@[0].run == if case >= 6 { Some(0u64) } else { None },
        result.nodes@[0].pos == 0,
        result.nodes@[0].question.unwrap().kind == if case == 0 { QKind::EffectFailed } else if case == 1 { QKind::CantMeet }
            else if case == 2 { QKind::OutOfCredit } else if case < 6 { QKind::NeedsSplitting } else { QKind::CleanupFailed },
        result.nodes@[0].question.unwrap().cont == if case == 0 { Cont::Ingesting } else if case <= 2 { Cont::Planning { must_split: false } } else { Cont::Nowhere },
        result.nodes@[0].question.unwrap().cost == if case == 1 || case == 2 { 1u8 } else { 0u8 },
        result.nodes@[0].generation == 0,
        result.nodes@[0].credit.start == 0, result.nodes@[0].credit.earned == 0,
] proof [reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(settled_world); reveal(movable);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.footprint = Footprint::inline(1);
    second.footprint = Footprint::inline(1);
    second.phase = Phase::Closed(Outcome::Stopped);
    first.phase = if case < 6 { Phase::AwaitingRob } else { Phase::Stopping(Outcome::Stopped) };
    let kind = if case == 0 { QKind::EffectFailed } else if case == 1 { QKind::CantMeet }
        else if case == 2 { QKind::OutOfCredit } else if case < 6 { QKind::NeedsSplitting }
        else { QKind::CleanupFailed };
    let cont = if case == 0 { Cont::Ingesting } else if case <= 2 { Cont::Planning { must_split: false } }
        else { Cont::Nowhere };
    let cost = if case == 1 || case == 2 { 1 } else { 0 };
    first.question = Some(Question { id: 0, kind, cont, cost, clarified: false });
    if case == 1 || case == 2 { first.credit.ready = true; }
    if case >= 6 { first.run = Some(0); first.pool = true; }
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 1,
        dependencies: Vec::new(), file_sets: Vec::new(), fail_fast: true };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(retirement::accepted(w));
        assert forall|i: int| 0 <= i < 2 implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(progress_phase_witness::original(w));
        assert forall|i: int| 0 <= i < 2 implies !movable(w, i) by { assert(i == 0 || i == 1); }
    }
    kernel_proof! {
        progress_phase::reserves(w);
        reveal(retirement_domain); reveal(retirement_accepted);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    kernel_proof! {
        reveal(storage_ordered);
        checked_reserves(w);
        progress_phase::zero_internal_height_definition(w);
        reveal_with_fuel(potential_prefix, 3);
        assert(progress_phase::zero_internal_height(w));
    }
    let settled = w.is_settled();
    assert!(settled);
    w
}
}

kernel_fn! {
/// #806 change request changed-world cover: construct an original-domain
/// consistent, settled world and run the real apply without settling afterward.
fn cover_change_request() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::ChangeRequest, 1),
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(handled); reveal(retirement::stopped);] {
    let w = world(3);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::ChangeRequest);
    (before, after, out)
}
}

kernel_fn! {
/// #806 stop changed-world cover: construct an original-domain
/// consistent, settled world and run the real apply without settling afterward.
fn cover_stop() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::Stop, 1),
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(handled); reveal(retirement::stopped);] {
    let w = world(4);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::Stop);
    (before, after, out)
}
}

kernel_fn! {
/// #806 unclear changed-world cover: construct an original-domain
/// consistent, settled world and run the real apply without settling afterward.
fn cover_unclear() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::Unclear, 1),
        result.1.nodes@[0].question.is_some(), result.1.nodes@[0].question.unwrap().id == 1,
        result.1.nodes@[0].question.unwrap().clarified,
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(handled); reveal(retirement::stopped);] {
    let w = world(5);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::Unclear);
    (before, after, out)
}
}

kernel_fn! {
/// #806 retry changed-world cover: construct an original-domain
/// consistent, settled world and run the real apply without settling afterward.
fn cover_retry() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::Retry, 1),
        result.1.nodes@[0].question.is_none(), result.1.nodes@[0].run == Some(0u64),
        result.1.nodes@[0].pos == 0,
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(handled); reveal(retirement::stopped);] {
    let w = world(6);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::Retry);
    (before, after, out)
}
}

kernel_fn! {
/// #806 mark_handled changed-world cover: construct an original-domain
/// consistent, settled world and run the real apply without settling afterward.
fn cover_mark_handled() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::MarkHandled, 1),
        result.1.nodes@[0].question.is_none(), result.1.nodes@[0].run.is_none(),
        result.1.nodes@[0].pos == 1,
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(handled); reveal(retirement::stopped);] {
    let w = world(7);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::MarkHandled);
    (before, after, out)
}
}

kernel_fn! {
/// #806 continue cover: construct an original-domain accepted, settled world and
/// execute real apply, including arbitrary u8 deposits and the zero-deposit top-up.
fn cover_continue() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::Continue, 1),
        result.1.nodes@[0].question.is_none(), result.1.nodes@[0].generation == 1,
        result.1.nodes@[0].phase == Phase::Ingesting,
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(retirement::stopped);] {
    let w = world(0);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::Continue);
    (before, after, out)
}
}

kernel_fn! {
/// #806 try_again cover: construct an original-domain accepted, settled world and
/// execute real apply, including arbitrary u8 deposits and the zero-deposit top-up.
fn cover_try_again() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::TryAgain, 1),
        result.1.nodes@[0].question.is_none(), result.1.nodes@[0].generation == 1,
        result.1.nodes@[0].phase == (Phase::Planning { must_split: false }),
        result.1.nodes@[0].attempt == 1, result.1.nodes@[0].round == 0,
        !result.1.nodes@[0].replanned, !result.1.nodes@[0].review_restarted,
        result.1.nodes@[0].credit.added == 1, result.1.nodes@[0].credit.spent == 1,
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(retirement::stopped);] {
    let w = world(1);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::TryAgain);
    (before, after, out)
}
}

kernel_fn! {
/// #806 add_credit cover: construct an original-domain accepted, settled world and
/// execute real apply, including arbitrary u8 deposits and the zero-deposit top-up.
fn cover_add_credit(k: u8) -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        retirement::accepted(result.1), result.1.nodes@[0] != result.0@.nodes@[0],
        result.1.nodes@[0] == commanded(result.0@.nodes@[0], 1, 0, Answer::AddCredit(k), 1),
        result.1.nodes@[0].question.is_none(), result.1.nodes@[0].generation == 1,
        result.1.nodes@[0].credit.added == if k == 0 { 1u64 } else { k as u64 },
        result.1.nodes@[0].credit.spent == 1,
] proof [reveal(funded); reveal(commanded); reveal(answered); reveal(continued); reveal(rephased); reveal(retirement::stopped);] {
    let w = world(2);
    kernel_snapshot! { before = w }
    let (after, out) = port_answers_apply::verify_answer(w, 1, 0, Answer::AddCredit(k));
    (before, after, out)
}
}
