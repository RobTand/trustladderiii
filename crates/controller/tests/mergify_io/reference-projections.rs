use super::*;
use tl3_mergify::decide::recovery::eligibility;
include!("reference-projection-support.rs");
struct Adapter;
impl Adapter {
    pub(super) fn repaired_head(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let Some(snapshot) = snapshot else {
            return Ok(false);
        };
        let world: World = super::snapshot::decode(snapshot)?;
        let Some(node) = world.nodes.get(delivery.node().0 as usize) else {
            return Ok(false);
        };
        let (number, head) = enqueue(delivery)?;
        if !heal_phase(Some(node.phase))
            || node.paused
            || node.waiting_incident
            || !node.pr_open
            || node.head != node.approved
            || node.checked != node.approved
            || approved_head(snapshot, node.approved)? != head
            || super::request(snapshot, delivery.node())?.repo != delivery.repository
            || !snapshot.rows.iter().any(|row| {
                row.table == "pull_requests"
                    && row.id == delivery.node().0.to_string()
                    && row.value["number"] == number
            })
        {
            return Ok(false);
        }
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = super::decode_stored(row.value.clone())?;
            if fact.repository == delivery.repository
                && fact.number == number
                && matches!(fact.evidence,
                    Evidence::CiFailed { head: ref prior, head_check: false, .. }
                    | Evidence::Conflict { head: ref prior, .. } if prior != head)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn human_hold_in_events(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
        events: &[tl3_github::IssueActivity],
    ) -> Result<bool> {
        for fact in person_facts(&delivery.repository, enqueue(delivery)?.0, events, false) {
            if let Some(snapshot) = snapshot {
                if fact.unresolved_hold(snapshot)? {
                    return Ok(true);
                }
            } else {
                // Without a snapshot there is no proof of an explicit resume.
                return Ok(true);
            }
        }
        self.unresolved_human_hold(delivery, snapshot)
    }
    pub(super) fn unresolved_human_hold(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let Some(snapshot) = snapshot else {
            return Ok(false);
        };
        let (number, _) = enqueue(delivery)?;
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = super::decode_stored(row.value.clone())?;
            if fact.repository == delivery.repository
                && fact.number == number
                && fact.unresolved_hold(snapshot)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
pub(super) fn run(
    entry: Entry,
    delivery: &Delivery,
    snapshot: Option<&Snapshot>,
    events: &[IssueActivity],
) -> Result<bool> {
    match entry {
        Entry::Repair => Adapter.repaired_head(delivery, snapshot),
        Entry::Hold => Adapter.human_hold_in_events(delivery, snapshot, events),
    }
}
