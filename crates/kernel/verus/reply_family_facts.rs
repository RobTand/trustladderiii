//! Narrow facts for opaque retirement values used by reply producers.
use super::*;

verus! {
pub(super) proof fn stopped_review(n: Node, step: ReviewStop)
    requires n.generation < u64::MAX,
    ensures ({ let z = retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped);
        z.repo != n.repo && z.footprint == n.footprint && (settle_shape(n) ==> settle_shape(z))
        && z.generation as int <= n.generation as int + 1 && z.next_seq == n.next_seq
        && z.credit == n.credit && z.last_cmd == n.last_cmd && z.review_stop == Some(step)
        && reply_family_values::frames(n, z) && z.plan_retries == n.plan_retries
        && z.attempt == n.attempt && z.replanned == n.replanned && z.round == n.round
        && z.rechecks == n.rechecks && z.run_failures == n.run_failures && z.holds == n.holds
        && (!(n.phase is Closed || n.phase is Stopping) ==>
            z.phase == Phase::Stopping(Outcome::Stopped) && z.question.is_none()) }),
{
    reveal(retirement::stopped);
}

pub(super) proof fn incident_facts(n: Node, needs_rob: bool)
    requires holding(n.phase) ==> n.generation < u64::MAX,
    ensures ({ let z = retirement::incident(n, needs_rob);
        z.repo == n.repo && z.footprint == n.footprint && (settle_shape(n) ==> settle_shape(z))
        && z.generation as int == n.generation as int + if holding(n.phase) { 1int } else { 0int }
        && z.next_seq == n.next_seq && z.credit == n.credit && z.question == n.question
        && z.waiting_incident && z.incident_needs_rob == needs_rob && z.ticket.is_none()
        && z.run_failures == n.run_failures && z.run_incidents == n.run_incidents
        && z.fx_failures == n.fx_failures && z.fx_incidents == n.fx_incidents
        && z.last_cmd == n.last_cmd && reply_family_values::frames(n, z)
        && z.review_stop == n.review_stop && z.attempt == n.attempt && z.replanned == n.replanned }),
{
    reveal(retirement::incident);
}
}
