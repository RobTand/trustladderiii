#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(not(verus_keep_ghost))]
macro_rules! checked_while { (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body }; }
#[cfg(verus_keep_ghost)]
macro_rules! checked_while { (($condition:expr) [$($contract:tt)*] $body:block) => { vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body } }; }

#[cfg(verus_keep_ghost)]
vstd::prelude::verus! {
    pub open spec fn prefix(chosen: Seq<u8>, subject: Seq<u8>) -> bool {
        chosen.len() <= subject.len() && forall|i:int| 0 <= i < chosen.len() ==> chosen[i] == subject[i]
    }
    pub open spec fn covered(chosen: Seq<u8>, subject: Seq<u8>) -> bool {
        prefix(chosen,subject) && (chosen.len() == subject.len() ||
            chosen.len()+2 <= subject.len() && subject[chosen.len() as int] == 58 && subject[chosen.len() as int+1] == 58)
    }
}

/// #820 source proof applicability: a subject is equal to, or nested under, chosen.
/// Unbounded byte prefix comparison; `::` boundaries preserve UTF-8 string semantics.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == covered(chosen@, subject@)))]
pub fn covers(chosen: &[u8], subject: &[u8]) -> bool {
    if chosen.len() > subject.len() {
        return false;
    }
    let mut i = 0;
    checked_while! { (i<chosen.len()) [
        invariant i<=chosen.len(), chosen.len()<=subject.len(),
            forall|j:int| 0<=j<i ==> chosen[j]==subject[j],
        decreases chosen.len()-i,
    ] {
        if chosen[i]!=subject[i] { return false; }
        i+=1;
    }}
    chosen.len() == subject.len()
        || subject.len() - chosen.len() >= 2
            && subject[chosen.len()] == b':'
            && subject[chosen.len() + 1] != b':'
}
