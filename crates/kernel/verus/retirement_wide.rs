//! #806: full production consistency for append and four-node gate constructions.
use super::*;

verus! {
/// #806: covers both original two-node worlds, appends, and four-node gate worlds.
/// Empty prerequisites and no withheld credit are the original harness assumptions.
pub(super) open spec fn domain(w: World) -> bool {
    w.nodes@.len() <= 4 && w.gates@.len() > 0 && w.dependencies@.len() == 0
        && forall|i: int| 0 <= i < w.nodes@.len() ==> !w.nodes@[i].credit.withheld
}

/// State-table gate count, summed over the actual executable scan prefix.
pub(super) open spec fn holders(w: World, repo: RepoId, end: nat) -> nat {
    potential_prefix(w.nodes@, end, |n: Node| if n.holds && n.repo == repo { 1nat } else { 0nat })
}

/// State-table gate exclusion, in the real checker's earlier/later orientation.
pub(super) open spec fn pair(w: World, a: int, b: int) -> bool {
    w.nodes@[a].holds && w.nodes@[b].holds && w.nodes@[a].repo == w.nodes@[b].repo
        ==> footprint::disjoint(w, w.nodes@[a].footprint, w.nodes@[b].footprint)
}

/// State-table consistency: every production check, not only its progress projection.
#[verifier::opaque]
pub(super) open spec fn accepted(w: World) -> bool {
    w.gates@.len() > 0 && w.pool_capacity > 0
        && (forall|r: int| 0 <= r < w.gates@.len() ==> w.gates@[r].w > 0
            && holders(w, RepoId(r as u64), w.nodes@.len()) <= w.gates@[r].w)
        && (forall|i: int| 0 <= i < w.nodes@.len() ==> retirement::node_rules(w, w.nodes@[i]))
        && (forall|a: int, b: int| 0 <= a < b < w.nodes@.len() ==> pair(w, a, b))
        && pool_users(w.nodes@, w.nodes@.len()) <= w.pool_capacity
        && (forall|a: int, b: int| 0 <= a < w.nodes@.len() && 0 <= b < w.nodes@.len() && a != b
            ==> (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket))
}

/// State-table new-request/adoption rows: exact prefix and fresh-node frame.
pub(super) open spec fn creation(before: World, after: World, repo: RepoId,
    priority: Priority, head: Option<HeadId>) -> bool {
    after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@ && after.next_ticket == before.next_ticket
        && after.next_question == before.next_question && after.pool_capacity == before.pool_capacity
        && after.fail_fast == before.fail_fast
        && if repo.0 <= usize::MAX && repo.0 < before.gates@.len() {
            after.nodes@.len() == before.nodes@.len() + 1
                && (forall|i: int| 0 <= i < before.nodes@.len() ==> after.nodes@[i] == before.nodes@[i])
                && {
                    let n = after.nodes@[before.nodes@.len() as int];
                    n.proposed == head && n.priority == priority
                        && initial_node(Node { proposed: None, priority: Priority::Unranked, ..n }, repo)
                }
        } else { after.nodes@ == before.nodes@ }
}

/// #806: the widened complete checker agrees with the approved two-node predicate.
pub(super) proof fn two_node_equivalence(w: World)
    requires retirement::domain(w),
    ensures accepted(w) == retirement::accepted(w), domain(w),
{
    reveal(accepted); reveal(retirement::accepted);
    reveal_with_fuel(potential_prefix, 3);
    assert forall|r: int| 0 <= r < 2 implies
        #[trigger] holders(w, RepoId(r as u64), 2) == retirement::holders(w, RepoId(r as u64), 2) by {}
    assert(pool_users(w.nodes@, 2) == (if w.nodes@[0].pool { 1int } else { 0int })
        + (if w.nodes@[1].pool { 1int } else { 0int }));
    assert((forall|a: int, b: int| 0 <= a < b < 2 ==> pair(w, a, b)) == pair(w, 0, 1));
    assert((forall|a: int, b: int| 0 <= a < 2 && 0 <= b < 2 && a != b
        ==> (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket))
        == (w.nodes@[0].ticket.is_some() ==> w.nodes@[0].ticket != w.nodes@[1].ticket));
}
}

verus! {
/// State-table new-request/adoption rows / #806: appending an inert ingest node
/// cannot introduce holders, pool users, duplicate tickets or conflicting files.
pub(super) proof fn creation_safe(before: World, after: World, repo: RepoId,
    priority: Priority, head: Option<HeadId>)
    requires retirement::domain(before), retirement::accepted(before),
        creation(before, after, repo, priority, head),
    ensures domain(after), accepted(after),
{
    reveal(accepted); reveal(retirement::accepted); reveal(initial_node);
    reveal_with_fuel(potential_prefix, 4);
    retirement::node_frame(before, after, before.nodes@[0]);
    retirement::node_frame(before, after, before.nodes@[1]);
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
    if repo.0 <= usize::MAX && repo.0 < before.gates@.len() {
        let n = after.nodes@[2];
        footprint::inline_is_valid(after, 0);
        assert(retirement::node_rules(after, n)) by { reveal(retirement::node_rules); }
    }
    assert forall|r: int| 0 <= r < 2 implies
        #[trigger] holders(after, RepoId(r as u64), after.nodes@.len())
            == retirement::holders(before, RepoId(r as u64), 2) by {}
    assert(pool_users(after.nodes@, after.nodes@.len()) == (if before.nodes@[0].pool { 1int } else { 0int })
        + (if before.nodes@[1].pool { 1int } else { 0int }));
    assert forall|a: int, b: int| 0 <= a < b < after.nodes@.len() implies pair(after, a, b) by {}
}
}

kernel_fn! {
/// State-table creation rows / #806: real apply and full production check.
/// The original construction has two nodes; a valid repository appends a third.
/// Priority and proposed head are arbitrary, strengthening the original head7 proof.
pub(super) fn verify_new_apply(mut w: World, repo: RepoId, priority: Priority,
    head: Option<HeadId>) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
    ensures domain(result.0), accepted(result.0), creation(w, result.0, repo, priority, head),
        result.1.len == 0,
        forall|i: int| 0 <= i < w.nodes@.len() ==> result.0.nodes@[i] == w.nodes@[i],
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 4); reveal(storage_ordered);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(retirement_wide_domain); reveal(retirement_wide_accepted);
    reveal(retirement::accepted); reveal(retirement::bounded); reveal(creation); reveal(initial_node);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        dependencies::indexable_definition(w);
    }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    let mut out = Outbox::new();
    let event = match head {
        Some(head) => Event::NewAdoption { repo, priority, head },
        None => Event::NewRequest { repo, priority },
    };
    apply(&mut w, event, &mut out);
    kernel_proof! { creation_safe(before@, w, repo, priority, head); }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out)
}
}
