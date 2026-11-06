//! #806: exact results for acknowledgement and owner-command ports.
use super::*;

verus! {
/// State-table owner commands: Answers belong to the separate answer port.
pub(super) open spec fn owner_command(c: Command) -> bool {
    matches!(c, Command::Stop | Command::Pause | Command::Resume | Command::ChangeRequest)
}

/// State-table command rows: duplicate IDs stutter, including their output.
#[verifier::opaque]
pub(super) open spec fn command_node(n: Node, id: u64, c: Command) -> Node {
    if id <= n.last_cmd { n } else {
        let fresh = Node { last_cmd: id, ..n };
        match c {
            Command::Stop => retirement::stopped(fresh, Outcome::Stopped),
            Command::ChangeRequest => retirement::stopped(fresh, Outcome::Replaced),
            Command::Pause => if n.phase is Stopping || n.phase is Closed { fresh }
                else { Node { paused: true, ..fresh } },
            Command::Resume => Node { paused: false, ..fresh },
            _ => fresh,
        }
    }
}

/// #806: the allocation/storage frame common to these single-node transitions.
#[verifier::opaque]
pub(super) open spec fn world_frame(b: World, a: World, i: usize) -> bool {
    a.nodes@.len() == b.nodes@.len()
        && (forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j])
        && a.next_ticket == b.next_ticket && a.gates@ == b.gates@
        && a.file_sets@ == b.file_sets@ && a.dependencies@ == b.dependencies@
        && a.pool_capacity == b.pool_capacity && a.fail_fast == b.fail_fast
}

/// State-table owner command frame, including the complete unchanged outbox.
#[verifier::opaque]
pub(super) open spec fn command_post(b: World, a: World, i: usize, id: u64,
    c: Command, ob: Outbox, oa: Outbox) -> bool {
    world_frame(b, a, i) && a.next_question == b.next_question && oa == ob
        && a.nodes@[i as int] == command_node(b.nodes@[i as int], id, c)
}

/// #806: canonical empty output used by the executable apply theorem.
pub(super) open spec fn empty() -> Outbox {
    Outbox { items: [None; OUTBOX], len: 0 }
}

/// #806: every check_nodes assertion shared by the eight retired harnesses.
#[verifier::opaque]
pub(super) open spec fn node_transition(b: Node, a: Node) -> bool {
    a.proposed == b.proposed && (b.adopted ==> a.adopted) && a.next_seq >= b.next_seq
        && (b.phase is Closed ==> a.phase == b.phase)
        && a.credit.earned as int <= b.credit.earned as int + 1
        && (a.credit.earned > b.credit.earned ==> !b.credit.merge_credited && a.credit.merge_credited)
}

/// Q54 / #806: command and Ack common assertions, including the publication exception.
#[verifier::opaque]
pub(super) open spec fn q54(b: Node, a: Node, fail_fast: bool, publication: bool) -> bool {
    (publication ==> a.review_stop == Some(ReviewStop::PublicationBody))
        && (!fail_fast ==> a.review_stop == b.review_stop || publication)
        && (fail_fast ==> a.attempt <= if b.attempt > 1 { b.attempt } else { 1u8 })
        && (fail_fast ==> b.replanned || !a.replanned)
        && (fail_fast && a.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && (fail_fast ==> match a.question {
            Some(q) => q.kind != QKind::CantMeet || b.question == Some(q), None => true,
        })
        && (a.review_stop != b.review_stop ==> b.review_stop.is_none()
            && a.phase == Phase::Stopping(Outcome::Stopped) && a.credit == b.credit && a.question.is_none())
}
}
