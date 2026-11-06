//! #806 A1: executable covers in every retired phase, including parameters.
use super::progress_phase::*;
use super::*;
verus! {
/// #806 A1: precisely the seven phase families in this retirement slice.
pub(super) open spec fn phase_a1(phase: Phase) -> bool {
    matches!(phase, Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }
        | Phase::Ready(_) | Phase::Making | Phase::Checking | Phase::Reviewing)
}

/// #806: witnesses stay inside the original small-world constructor, including
/// small heads, four-file inline storage, every counter and auxiliary ID.
pub(super) open spec fn original(w: World) -> bool {
    domain(w) && w.pool_capacity <= 2
        && (forall|r: int| 0 <= r < 2 ==> w.gates@[r].w <= 3)
        && forall|i: int| 0 <= i < 2 ==> {
            let n = #[trigger] w.nodes@[i];
            n.repo.0 < 2 && n.footprint == footprint::inline_footprint(1)
                && n.credit.start < 1073741824u64 && n.credit.earned < 1073741824u64
                && n.credit.added < 1073741824u64 && n.credit.spent < 1073741824u64
                && (match n.proposed { Some(h) => h.0 < 8, None => true })
                && (match n.head { Some(h) => h.0 < 8, None => true })
                && (match n.checked { Some(h) => h.0 < 8, None => true })
                && (match n.approved { Some(h) => h.0 < 8, None => true })
                && (match n.merged { Some(h) => h.0 < 8, None => true })
                && (match n.ticket { Some(t) => t < 1099511627776u64, None => true })
                && (match n.run { Some(t) => t < 1099511627776u64, None => true })
                && (match n.job { Some(t) => t < 1099511627776u64, None => true })
                && (match n.effect { Some(e) => e.seq < 1099511627776u64, None => true })
                && (match n.question { Some(q) => q.id < 1099511627776u64, None => true })
        }
}
}

kernel_fn! {
/// Q30/Q39 / #806 A1: every supplied phase, both split flags and every Resume,
/// has a consistent settled original-domain world with an open node.
fn world(phase: Phase) -> (result: World) [
    requires phase_a1(phase),
    ensures original(result), retirement::accepted(result), settled_world(result),
        result.nodes@[0].phase == phase, !(result.nodes@[0].phase is Closed),
] proof [reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = phase;
    first.paused = true;
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    match phase {
        Phase::Making => {
            first.holds = true;
            first.pos = 2;
            first.run = Some(0);
            first.pool = true;
        },
        Phase::Checking => {
            first.holds = true;
            first.pos = 1;
            first.job = Some(0);
        },
        Phase::Reviewing => {
            first.holds = true;
            first.pos = 1;
            first.run = Some(0);
            first.pool = true;
        },
        _ => {},
    }
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        dependencies: Vec::new(), file_sets: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(retirement::accepted(w));
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(domain(w));
        assert(original(w));
        reveal(settled_world); reveal(movable);
        assert forall|i: int| 0 <= i < w.nodes@.len() implies !movable(w, i) by {
            assert(i == 0 || i == 1);
        }
    }
    scope(&w);
    kernel_proof! {
        reveal(progress_scope); reveal(storage_ordered);
        checked_reserves(w);
        zero_internal_height_definition(w);
        reveal_with_fuel(potential_prefix, 3);
        assert(potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) == 0);
    }
    let settled = w.is_settled();
    assert!(settled);
    w
}
}

kernel_fn! {
/// #806 A1 cover: construct the requested phase, then call real witness/apply.
/// No settle occurs. One constructor covers all original phase parameters.
fn cover(phase: Phase) -> (result: (Ghost<World>, World, Event, u64, u64)) [
    requires phase_a1(phase),
    ensures original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        result.0@.nodes@[0].phase == phase,
        witness_selection(result.0@, Some(result.2)), progress_input(result.0@, result.2),
        result.3 as nat == progress_measure(result.0@),
        result.4 as nat == progress_measure(result.1), result.4 < result.3,
] {
    let w = world(phase);
    kernel_snapshot! { before = w }
    let (after, event, old_measure, new_measure) = verify_apply(w);
    (before, after, event, old_measure, new_measure)
}
}
