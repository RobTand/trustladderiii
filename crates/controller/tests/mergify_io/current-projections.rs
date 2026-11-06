use super::*;
use tl3_mergify::decide::recovery::eligibility;
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
        let mut repaired = eligibility::Repaired {
            phase: heal_phase(Some(node.phase)),
            paused: node.paused,
            incident: node.waiting_incident,
            open: node.pr_open,
            head_approved: node.head == node.approved,
            checked_approved: node.checked == node.approved,
            approved_head: None,
            repository: None,
            pull_request: None,
        };
        loop {
            match eligibility::repaired_head(repaired) {
                eligibility::RepairStep::Stop => return Ok(false),
                eligibility::RepairStep::ApprovedHead => {
                    repaired.approved_head = Some(approved_head(snapshot, node.approved)? == head);
                }
                eligibility::RepairStep::Repository => {
                    repaired.repository = Some(
                        super::request(snapshot, delivery.node())?.repo == delivery.repository,
                    );
                }
                eligibility::RepairStep::PullRequest => {
                    repaired.pull_request = Some(snapshot.rows.iter().any(|row| {
                        row.table == "pull_requests"
                            && row.id == delivery.node().0.to_string()
                            && row.value["number"] == number
                    }));
                }
                eligibility::RepairStep::History => break,
            }
        }
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = super::decode_stored(row.value.clone())?;
            let (failure, other_head) = match &fact.evidence {
                Evidence::CiFailed {
                    head: prior,
                    head_check: false,
                    ..
                } => (eligibility::PriorFailure::QueueCi, prior != head),
                Evidence::Conflict { head: prior, .. } => {
                    (eligibility::PriorFailure::Conflict, prior != head)
                }
                _ => (eligibility::PriorFailure::Other, false),
            };
            if eligibility::prior_failure(
                fact.repository == delivery.repository,
                fact.number == number,
                failure,
                other_head,
            ) {
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
            if eligibility::event_hold(
                snapshot
                    .map(|snapshot| fact.unresolved_hold(snapshot))
                    .transpose()?,
            ) {
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
