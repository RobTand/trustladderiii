//! Q39 / #806 W7: independent priority-level/ticket order and real pool queries.
use super::*;

verus! {
/// State-table pool order, written independently of priority_number and rank.
pub(super) open spec fn spec_key(priority: Priority, ticket: u64) -> (nat, u64) {
    (match priority { Priority::P0 => 0, Priority::P1 => 1, Priority::P2 => 2,
        Priority::P3 => 3, Priority::Unranked => 4 }, ticket)
}

/// Q39: lexicographic order compares priority level before ticket age.
pub(super) open spec fn key_less(a: (nat, u64), b: (nat, u64)) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1 < b.1)
}

/// Q39 / #806: the reference pool winner has a strictly smaller independent
/// key than every other ticketed pool waiter. Ready nodes use the slot line.
pub(super) open spec fn best(w: World, i: int) -> bool {
    w.nodes@[i].ticket.is_some()
        && forall|j: int| 0 <= j < w.nodes@.len() && j != i
            && !(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some()
            ==> key_less(spec_key(w.nodes@[i].priority, w.nodes@[i].ticket.unwrap()),
                spec_key(w.nodes@[j].priority, w.nodes@[j].ticket.unwrap()))
}

/// #806: every declared Priority has its independent state-table level.
pub(super) proof fn priority_level(a: Priority, b: Priority)
    ensures priority_number(a) == spec_key(a, 0).0,
        priority_number(b) == spec_key(b, 0).0,
        (a == b) == (spec_key(a, 0).0 == spec_key(b, 0).0),
{
    match a { Priority::P0 => {}, Priority::P1 => {}, Priority::P2 => {}, Priority::P3 => {}, Priority::Unranked => {} }
    match b { Priority::P0 => {}, Priority::P1 => {}, Priority::P2 => {}, Priority::P3 => {}, Priority::Unranked => {} }
}

/// #806: the real earlier_in_pool decision equals the independent spec_key
/// for every phase, optional ticket, priority, and full-width ticket value.
pub(super) fn order_matches(a: Node, b: Node) -> (result: bool)
    ensures result == (!(a.phase is Ready) && a.ticket.is_some() && b.ticket.is_some()
        && key_less(spec_key(a.priority, a.ticket.unwrap()), spec_key(b.priority, b.ticket.unwrap()))),
{
    let result = earlier_in_pool(&a, &b);
    proof { priority_level(a.priority, b.priority); }
    result
}

/// #806: actual scheduler order is strict priority, then ticket age.
pub(super) fn strict_priority(a: Node, b: Node) -> (result: bool)
    requires a.phase == Phase::Ingesting, b.phase == Phase::Ingesting,
        a.ticket.is_some(), b.ticket.is_some(), a.ticket != b.ticket,
    ensures result == key_less(spec_key(a.priority, a.ticket.unwrap()), spec_key(b.priority, b.ticket.unwrap())),
{
    let actual = earlier_in_pool(&a, &b);
    proof { priority_level(a.priority, b.priority); }
    actual
}

/// Q39 / #806: the production oldest-waiter predicate equals the independent
/// strict key minimum, for arbitrary finite storage and full-width tickets.
pub(super) proof fn line_definition(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), !(w.nodes@[i].phase is Ready), w.nodes@[i].ticket.is_some(),
        forall|j: int| 0 <= j < w.nodes@.len() && j != i && w.nodes@[j].ticket.is_some()
            ==> w.nodes@[j].ticket != w.nodes@[i].ticket,
    ensures oldest_waiter(w, i) == best(w, i),
{
    reveal(oldest_waiter); reveal(pool_precedes);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies
        !pool_precedes(w.nodes@[j], w.nodes@[i])
            == (j == i || w.nodes@[j].phase is Ready || w.nodes@[j].ticket.is_none()
                || key_less(spec_key(w.nodes@[i].priority, w.nodes@[i].ticket.unwrap()),
                    spec_key(w.nodes@[j].priority, w.nodes@[j].ticket.unwrap()))) by {
        priority_level(w.nodes@[i].priority, w.nodes@[j].priority);
    }
}

/// Q39 / #806: run the real complete first-in-line scan and compare its result
/// to the independent state-table order. The table length is unrestricted.
pub(super) fn line_query(w: &World, i: usize) -> (result: bool)
    requires i < w.nodes@.len(), scheduling_safe(*w),
        !(w.nodes@[i as int].phase is Ready), w.nodes@[i as int].ticket.is_some(),
        forall|j: int| 0 <= j < w.nodes@.len() && j != i && w.nodes@[j].ticket.is_some()
            ==> w.nodes@[j].ticket != w.nodes@[i as int].ticket,
    ensures result == best(*w, i as int),
{
    let result = first_in_line(w, i);
    proof { line_definition(*w, i as int); }
    result
}

}
