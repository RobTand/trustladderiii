//! New-family acceptance projection, isolated from the producing entries.
use super::*;

verus! {

pub(super) proof fn new_accepted_rows(before: World, after: World, event: Event)
    requires new_acceptance::new_accepted(before, after, event),
    ensures (checking::accepted(before) && (event is NewRequest || event is NewAdoption))
        ==> checking::accepted(after),
{
    reveal(new_acceptance::new_accepted);
}

}
