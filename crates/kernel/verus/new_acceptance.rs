//! Acceptance for the real NewRequest/NewAdoption entries, via the checker's C2.
use super::*;

verus! {

/// Per-field posts of the New* entries; valid repositories append, invalid ones stutter.
pub(super) proof fn new_world_accepted(before: World, after: World, repo: RepoId)
    requires checking::accepted(before),
        forall|j: int| 0 <= j < before.nodes@.len() ==> after.nodes@[j] == before.nodes@[j],
        after.next_ticket == before.next_ticket,
        after.next_question == before.next_question,
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        (repo.0 < before.gates@.len() ==> after.nodes@.len() == before.nodes@.len() + 1),
        (repo.0 >= before.gates@.len() ==> after.nodes@ == before.nodes@),
        (repo.0 < before.gates@.len() ==> {
            let n = after.nodes@[before.nodes@.len() as int];
            retirement::node_rules(after, n) && !n.holds && !n.pool
                && n.ticket.is_none() && !n.credit.withheld
        }),
    ensures checking::accepted(after),
{
    if repo.0 < before.gates@.len() {
        internal_safety::append_preserves_accepted(before, after);
    } else {
        checking::accepted_frame(before, after);
    }
}

/// ONE opaque New-family clause carried by events::apply_proved and apply.
#[verifier::opaque]
pub(super) open spec fn new_accepted(before: World, after: World, event: Event) -> bool {
    match event {
        Event::NewRequest { .. } | Event::NewAdoption { .. } =>
            checking::accepted(before) ==> checking::accepted(after),
        _ => true,
    }
}

pub(super) proof fn new_accepted_intro(before: World, after: World, event: Event)
    requires match event {
        Event::NewRequest { .. } | Event::NewAdoption { .. } =>
            checking::accepted(before) ==> checking::accepted(after),
        _ => true,
    },
    ensures new_accepted(before, after, event),
{
    reveal(new_accepted);
}

}
