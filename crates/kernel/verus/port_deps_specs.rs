//! #693 / #806 W7: source-connected prerequisite frames and checker premises.
use super::*;

verus! {
/// State-table prerequisite identity is owner, kind, and external number.
pub(super) open spec fn key(d: Dependency, i: int, target: DependencyTarget) -> bool {
    d.node == NodeId(i as u64) && d.target.kind == target.kind && d.target.number == target.number
}

/// #806: a retained Pending declaration accepts either Delivered or Abandoned observations.
pub(super) open spec fn pending_key(w: World, i: int, target: DependencyTarget) -> bool {
    target.number > 0
        && (match target.request { None => true, Some(n) => n.0 < w.nodes@.len()
            && w.nodes@[n.0 as int].repo == w.nodes@[i].repo })
        && (exists|k: int| 0 <= k < w.dependencies@.len() && key(w.dependencies@[k], i, target))
        && (forall|k: int| 0 <= k < w.dependencies@.len() && key(w.dependencies@[k], i, target)
            ==> w.dependencies@[k].target == target && !w.dependencies@[k].rejected
                && w.dependencies@[k].status == DependencyStatus::Pending)
}

/// State-table prerequisite registration / #806: the original handler guards,
/// with an absent declaration key and an owner that has not begun planning.
pub(super) open spec fn new_key(w: World, i: int, target: DependencyTarget) -> bool {
    target.number > 0 && !w.nodes@[i].planning_started
        && (w.nodes@[i].phase == Phase::Ingesting || w.nodes@[i].phase == Phase::Auditing
            || w.nodes@[i].phase == Phase::Unplanned)
        && (match target.request { None => true, Some(n) => n.0 < w.nodes@.len()
            && w.nodes@[n.0 as int].repo == w.nodes@[i].repo })
        && forall|k: int| 0 <= k < w.dependencies@.len() ==> !key(w.dependencies@[k], i, target)
}

/// #806: exact rejection decision for the verified u64 graph representation.
pub(super) open spec fn cycle_rejected(w: World, i: int, target: DependencyTarget,
    status: DependencyStatus) -> bool {
    status == DependencyStatus::Pending && match target.request {
        None => false, Some(to) => port_deps_graph::reaches(w, to, NodeId(i as u64)) }
}

/// #806: all World storage and scalar fields, independent of Vec allocation.
pub(super) open spec fn unchanged(a: World, b: World) -> bool {
    a.nodes@ == b.nodes@ && a.dependencies@ == b.dependencies@ && a.gates@ == b.gates@
        && a.file_sets@ == b.file_sets@ && a.pool_capacity == b.pool_capacity
        && a.next_ticket == b.next_ticket && a.next_question == b.next_question && a.fail_fast == b.fail_fast
}

/// State-table prerequisite observation: the real handler changes
/// exactly one matching table row, with no bound on other declarations.
#[verifier::opaque]
pub(super) open spec fn resolved(before: World, after: World, i: int,
    target: DependencyTarget, status: DependencyStatus) -> bool {
    exists|k: int| 0 <= k < before.dependencies@.len() && key(before.dependencies@[k], i, target)
        && before.dependencies@[k].status == DependencyStatus::Pending
        && after.dependencies@ == before.dependencies@.update(k, Dependency {
            node: NodeId(i as u64), target, status, rejected: false })
}

/// State-table abandoned/cyclic prerequisite rows: exact appended note or silence.
#[verifier::opaque]
pub(super) open spec fn recorded_output(before: Outbox, after: Outbox, i: usize, d: Dependency) -> bool {
    if d.rejected || d.status == DependencyStatus::Abandoned {
        after.len == before.len + 1
            && after.items@ == before.items@.update(before.len as int, Some(Out::Notice {
                node: NodeId(i as u64), notice: Notice::Note(if d.rejected {
                    NoteKind::DependencyCycle { number: d.target.number }
                } else { NoteKind::DependencyAbandoned { number: d.target.number } }) }))
    } else { after == before }
}

/// #806: exact dependency handler postcondition propagated through real apply.
#[verifier::opaque]
pub(super) open spec fn handler_post(before: World, after: World, i: int,
    target: DependencyTarget, status: DependencyStatus, old_out: Outbox, out: Outbox) -> bool {
    dependencies::dependency_frame(before, after, i)
        && (before.nodes@[i].credit.withheld ==> after.nodes@[i].credit.withheld)
        && dependencies::edges_indexable(after)
        && (before.nodes@.len() <= 64 && port_deps_graph::acyclic(before) ==> port_deps_graph::acyclic(after))
        && (before.nodes@.len() <= 64 && new_key(before, i, target)
            ==> after.dependencies@ == before.dependencies@.push(Dependency {
                node: NodeId(i as u64), target, status,
                rejected: cycle_rejected(before, i, target, status) }))
        && (pending_key(before, i, target) && status != DependencyStatus::Pending ==> {
            resolved(before, after, i, target, status)
                && recorded_output(old_out, out, i as usize, Dependency {
                    node: NodeId(i as u64), target, status, rejected: false })
                && after.nodes@[i].waiting_prerequisite.is_none()
                    == dependencies::prerequisites_delivered(after, NodeId(i as u64))
        })
}

/// #806: original prerequisite constructors have one external declaration.
/// The universal graph/waiting theorems do not inherit this witness bound.
pub(super) open spec fn checker_dependencies(w: World) -> bool {
    if w.dependencies@.len() == 0 {
        forall|i: int| 0 <= i < w.nodes@.len() ==> !w.nodes@[i].credit.withheld
    } else {
        w.dependencies@.len() == 1 && w.nodes@.len() > 0
            && w.dependencies@[0].node == NodeId(0)
            && w.dependencies@[0].target.number > 0
            && w.dependencies@[0].target.request.is_none() && !w.dependencies@[0].rejected
            && (forall|i: int| 1 <= i < w.nodes@.len() ==> !w.nodes@[i].credit.withheld)
            && (w.dependencies@[0].status != DependencyStatus::Delivered ==> {
                let n = w.nodes@[0];
                !n.planning_started && n.phase == Phase::Unplanned && n.question.is_none()
                    && !n.holds && !n.pool && n.ticket.is_none() && n.run.is_none()
                    && dependencies::zero_credit(n.credit)
            })
    }
}

/// #806: complete checker acceptance for arbitrary finite tables without slot
/// holders. This includes both original one-root prerequisites and pool fixtures.
#[verifier::opaque]
pub(super) open spec fn checker_accepted(w: World) -> bool {
    w.gates@.len() > 0 && w.pool_capacity > 0 && checker_dependencies(w)
        && (forall|r: int| 0 <= r < w.gates@.len() ==> w.gates@[r].w > 0)
        && (forall|i: int| 0 <= i < w.nodes@.len()
            ==> retirement::node_rules(w, w.nodes@[i]) && !w.nodes@[i].holds)
        && pool_users(w.nodes@, w.nodes@.len()) <= w.pool_capacity
        && (forall|i: int, j: int| 0 <= i < j < w.nodes@.len() && w.nodes@[i].ticket.is_some()
            ==> w.nodes@[i].ticket != w.nodes@[j].ticket)
}

/// #693 / #806: two local wait-flag frames compose without re-expanding the
/// dependency table, output slots, or unrelated nodes in the handler proof.
pub(super) proof fn frame_transitive(before: World, middle: World, after: World, i: int)
    requires 0 <= i < before.nodes@.len(),
        dependencies::dependency_frame(before, middle, i), dependencies::dependency_frame(middle, after, i),
    ensures dependencies::dependency_frame(before, after, i),
{
    reveal(dependencies::dependency_frame);
}
}
