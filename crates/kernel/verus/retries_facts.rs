//! Failed-run branch facts, separate from mutable retry bookkeeping.
use super::*;

verus! {
/// Q30: abandoning an operation releases ownership and cancels its pending start.
#[verifier::inline]
pub(super) open spec fn abandoned(n: Node) -> Node {
    Node { run: None, job: None, pool: false,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n }
}

/// Q50: hide the complete failed-run update while proving each branch.
/// Must equal `port_t860_contracts::failed_node` for `run_failures <= 1`.
/// `failed_result` proves the connection through `failed_post`.
#[verifier::opaque]
pub(super) open spec fn failed_run(n: Node) -> Node {
    let m = Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
        generation: (n.generation + 1) as u64,
        run_failures: (n.run_failures + 1) as u8, ..abandoned(n) };
    if m.run_failures >= 2 {
        let incident = Node { run_failures: 0,
            run_incidents: if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 }, ..m };
        retirement::incident(incident, incident.run_incidents >= 2)
    } else { m }
}

/// Q50 failed-run row: the first failure restarts the same operation once.
pub(super) proof fn restarted(before: Node, after: Node)
    requires before.run_failures == 0,
        after == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 1, ..abandoned(before)
        }),
    ensures after == failed_run(before),
{
    reveal(abandoned);
    reveal(failed_run);
}

/// Q50 failed-run row: fold the saturated incident before mutating the world.
pub(super) proof fn incident(before: Node, ready: Node, needs_rob: bool)
    requires before.run_failures == 1, before.run_incidents <= 2,
        ready == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 0,
            run_incidents: if before.run_incidents < 2 { (before.run_incidents + 1) as u8 } else { 2u8 },
            ..abandoned(before)
        }),
        needs_rob == (ready.run_incidents >= 2),
    ensures retirement::incident(ready, needs_rob) == failed_run(before),
{
    reveal(abandoned);
    reveal(failed_run);
}

/// Q50: package the exact result and notice frame without exposing either to callers.
pub(super) proof fn failed_result(before: Node, after: Node, bo: Outbox, ao: Outbox)
    requires before.run_failures <= 1, after == failed_run(before),
        retirement::notices_only(bo) ==> retirement::notices_only(ao),
    ensures port_t860_contracts::failed_post(before, after, bo, ao),
{
    reveal(port_t860_contracts::failed_post);
    reveal(port_t860_contracts::failed_node);
    reveal(failed_run);
}

/// Keep the sequence write and incident result behind one private boundary.
#[verifier::opaque]
pub(super) open spec fn incident_write(b: Seq<Node>, a: Seq<Node>,
    i: int, needs_rob: bool) -> bool {
    a == node_writes::write(b, i, retirement::incident(b[i], needs_rob))
}

pub(super) proof fn pack_incident_write(b: Seq<Node>, a: Seq<Node>,
    i: int, needs_rob: bool)
    requires 0 <= i < b.len(), a.len() == b.len(),
        forall|j: int| 0 <= j < b.len() && j != i ==> a[j] == b[j],
        a[i] == retirement::incident(b[i], needs_rob),
    ensures incident_write(b, a, i, needs_rob),
{
    node_writes::write_from_frame(b, a, i);
    reveal(incident_write);
}

/// Supply only the failed-result and storage facts needed by run_failed.
pub(super) proof fn finish_failed_incident(before: Node,
    ready: Seq<Node>, after: Seq<Node>, i: int, needs_rob: bool,
    bo: Outbox, ao: Outbox)
    requires 0 <= i < ready.len(), before.run_failures <= 1,
        incident_write(ready, after, i, needs_rob),
        retirement::incident(ready[i], needs_rob) == failed_run(before),
        retirement::notices_only(bo) ==> retirement::notices_only(ao),
    ensures port_t860_contracts::failed_post(before, after[i], bo, ao),
        after.len() == ready.len(),
        forall|j: int| 0 <= j < ready.len() && j != i
            ==> after[j] == ready[j],
{
    reveal(incident_write);
    let value = retirement::incident(ready[i], needs_rob);
    node_writes::write_length(ready, i, value);
    node_writes::write_index(ready, i, value);
    node_writes::write_other(ready, i, value);
    failed_result(before, after[i], bo, ao);
}
}
