//! #823: reference reduction over exact serialized candidate sizes.
//! Documents consist of immutable mandatory atoms plus optional atoms sorted by
//! (priority, traversal position). Candidate i removes exactly that prefix.
//! The conformance adapter constructs those documents independently of bounded().
//! No input-length bound: the loop scans any finite slice, using no size sums.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! reference_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! reference_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

/// #823: 0 original, 1 ordered prefix, 2 mandatory core, 3 diagnostic, 4 error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Plan {
    pub kind: u8,
    pub removed: usize,
    pub bytes: u64,
}

/// #823(a,d): every success fits; a diagnostic/error requires oversized core.
/// Prefix sizes include all notices/commas/escapes from the envelope serializer.
/// No monotonicity assumption: the reference selects the first fitting prefix.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => requires core <= full,
    ensures out.kind <= 4,
        out.kind < 4 ==> out.bytes <= limit,
        (out.kind == 0) == (full <= limit),
        out.kind == 0 ==> out.bytes == full,
        (out.kind == 3 || out.kind == 4) == (core > limit),
        out.kind == 3 ==> out.bytes == diagnostic,
        out.kind == 4 ==> diagnostic > limit,
        out.kind == 2 ==> out.bytes == core && out.removed == prefix_sizes.len()
            && forall|j: int| 0 <= j < prefix_sizes.len() ==> prefix_sizes[j] > limit,
        out.kind == 1 ==> out.removed < prefix_sizes.len()
            && out.bytes == prefix_sizes[out.removed as int]
            && forall|j: int| 0 <= j < out.removed ==> prefix_sizes[j] > limit,
))]
pub fn reduce(full: u64, core: u64, diagnostic: u64, prefix_sizes: &[u64], limit: u64) -> Plan {
    if full <= limit {
        return Plan {
            kind: 0,
            removed: 0,
            bytes: full,
        };
    }
    if core > limit {
        return Plan {
            kind: if diagnostic <= limit { 3 } else { 4 },
            removed: 0,
            bytes: diagnostic,
        };
    }
    let mut i = 0;
    reference_while! { (i < prefix_sizes.len()) [
        invariant i <= prefix_sizes.len(), full > limit, core <= limit, core <= full,
            forall|j: int| 0 <= j < i ==> prefix_sizes[j] > limit,
        decreases prefix_sizes.len() - i,
    ] {
        if prefix_sizes[i] <= limit {
            return Plan { kind: 1, removed: i, bytes: prefix_sizes[i] };
        }
        i += 1;
    }}
    Plan {
        kind: 2,
        removed: prefix_sizes.len(),
        bytes: core,
    }
}

/// #823(b): unique traversal positions make the removal priority a total order.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (rank_a < rank_b || rank_a == rank_b && order_a < order_b),
    rank_a != rank_b || order_a != order_b ==>
        out != (rank_b < rank_a || rank_b == rank_a && order_b < order_a),
))]
pub fn precedes(rank_a: u8, order_a: usize, rank_b: u8, order_b: usize) -> bool {
    rank_a < rank_b || rank_a == rank_b && order_a < order_b
}

/// #823(c): mandatory proof/totals/RED atoms are retained in every non-diagnostic.
/// Other atoms survive exactly when they are outside the removed prefix.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (plan.kind == 0 || plan.kind <= 2 &&
        (mandatory || plan.kind == 1 && position >= plan.removed)),
    plan.kind <= 2 && mandatory ==> out,
    plan.kind >= 3 ==> !out,
))]
pub fn keeps(plan: Plan, position: usize, mandatory: bool) -> bool {
    plan.kind == 0 || plan.kind <= 2 && (mandatory || plan.kind == 1 && position >= plan.removed)
}

/// #823(b): retaining a lower-priority display atom retains every later atom.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => requires earlier <= later,
    ensures out,
))]
pub fn prefix_preserves_order(plan: Plan, earlier: usize, later: usize) -> bool {
    !keeps(plan, earlier, false) || keeps(plan, later, false)
}
