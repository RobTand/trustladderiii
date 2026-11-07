//! #693 / #771: opaque dependency-table writes and their consumed projections.
use super::*;

verus! {
#[verifier::opaque]
pub(super) open spec fn write(before: Seq<Dependency>, after: Seq<Dependency>,
    existing: Option<usize>, dependency: Dependency) -> bool {
    after == match existing {
        Some(index) => before.update(index as int, dependency),
        None => before.push(dependency),
    }
}

/// Fold the exact replacement or append at the private executable producer.
pub(super) proof fn pack(before: Seq<Dependency>, after: Seq<Dependency>,
    existing: Option<usize>, dependency: Dependency)
    requires match existing { None => true, Some(index) => index < before.len() },
        after == match existing {
            Some(index) => before.update(index as int, dependency),
            None => before.push(dependency),
        },
    ensures write(before, after, existing, dependency),
{
    reveal(write);
}

/// Storage length for the caller's actual replacement or append.
pub(super) proof fn length(before: Seq<Dependency>, after: Seq<Dependency>,
    existing: Option<usize>, dependency: Dependency)
    requires match existing { None => true, Some(index) => index < before.len() },
        write(before, after, existing, dependency),
    ensures match existing {
        Some(_) => after.len() == before.len(),
        None => after.len() == before.len() + 1,
    },
{
    reveal(write);
}

/// Recover the written row without exposing the whole sequence update.
pub(super) proof fn index(before: Seq<Dependency>, after: Seq<Dependency>,
    existing: Option<usize>, dependency: Dependency)
    requires match existing { None => true, Some(index) => index < before.len() },
        write(before, after, existing, dependency),
    ensures after[match existing { Some(index) => index as int, None => before.len() as int }]
        == dependency,
{
    reveal(write);
}

/// Every old row other than the replacement is preserved, with no status restriction.
pub(super) proof fn frame(before: Seq<Dependency>, after: Seq<Dependency>,
    existing: Option<usize>, dependency: Dependency)
    requires match existing { None => true, Some(index) => index < before.len() },
        write(before, after, existing, dependency),
    ensures forall|j: int| 0 <= j < before.len()
        && (match existing { Some(index) => j != index as int, None => true })
        ==> after[j] == before[j],
{
    reveal(write);
}

/// Recover the unchanged record contract from the narrow table projections.
pub(super) proof fn finish(before: Seq<Dependency>, after: Seq<Dependency>,
    existing: Option<usize>, dependency: Dependency)
    requires match existing { None => true, Some(index) => index < before.len() },
        write(before, after, existing, dependency),
    ensures after == match existing {
        Some(index) => before.update(index as int, dependency),
        None => before.push(dependency),
    },
{
    length(before, after, existing, dependency);
    index(before, after, existing, dependency);
    frame(before, after, existing, dependency);
    match existing {
        Some(index) => { assert(after =~= before.update(index as int, dependency)); }
        None => { assert(after =~= before.push(dependency)); }
    }
}
}
