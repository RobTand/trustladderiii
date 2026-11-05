//! Q30 / ingest rows, #771: isolate full-node record composition from real updates.
use super::*;

verus! {
/// Ingest-to-audit row / #771: deposit and phase change compose to the original frame.
pub(super) proof fn ingest_frame(before: Node, deposited: Node, after: Node, size: u8, early_end: Option<EarlyEnd>)
    requires before.generation < u64::MAX,
        deposited == (Node {
            credit: Credit { ready: true,
                start: if before.credit.ready { before.credit.start } else { (size as u64 + RESERVE) as u64 },
                ..before.credit }, early_end, ..before
        }),
        after == (Node {
            generation: (deposited.generation + 1) as u64,
            phase: Phase::Auditing, pos: 0, ticket: None,
            effect: match deposited.effect {
                Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
            }, ..deposited
        }),
    ensures after == (Node {
        credit: Credit { ready: true,
            start: if before.credit.ready { before.credit.start } else { (size as u64 + RESERVE) as u64 },
            ..before.credit },
        early_end, phase: Phase::Auditing, pos: 0, ticket: None,
        generation: (before.generation + 1) as u64,
        effect: match before.effect {
            Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        },
        ..before
    }),
{
}

/// Q30 pause row / #771: release and phase change retain the original holding frame.
pub(super) proof fn pause_frame(before: Node, released: Node, after: Node, r: Resume)
    requires holding(before.phase), before.generation < u64::MAX,
        Some(r) == match before.phase {
            Phase::Making => Some(Resume::Making), Phase::Checking => Some(Resume::Checking),
            Phase::Reviewing => Some(Resume::Reviewing),
            Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
            Phase::Rebasing => Some(Resume::Rebasing), _ => None,
        },
        released == (Node { holds: false, ..before }),
        after == (Node {
            generation: (released.generation + 1) as u64,
            phase: Phase::Ready(r), pos: 0, ticket: None,
            effect: match released.effect {
                Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
            }, ..released
        }),
    ensures after.generation as int <= before.generation as int + 1,
        after.repo == before.repo, after.footprint == before.footprint,
        after.pool == before.pool,
        after.next_seq == before.next_seq, after.credit == before.credit,
        after.question == before.question,
        after.paused == before.paused, after.waiting_incident == before.waiting_incident,
        if holding(before.phase) {
            after.phase == Phase::Ready(match before.phase {
                Phase::Making => Resume::Making,
                Phase::Checking => Resume::Checking,
                Phase::Reviewing => Resume::Reviewing,
                Phase::Publishing | Phase::Queued => Resume::Publishing,
                _ => Resume::Rebasing,
            }) && after.pos == 0 && after.ticket.is_none()
                && after.effect == match before.effect {
                    Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                }
        } else {
            before.ticket.is_some()
                && after == (Node { ticket: None, ..before })
        },
        after.last_cmd == before.last_cmd,
        after.holds == if holding(before.phase) { false } else { before.holds },
{
}
}
