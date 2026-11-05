//! Nondeterministic upstream composition for one original effect key. Reads,
//! request issue, application, repair writes and lost responses are distinct.
//! A scheduler chooses any enabled step; timeout never removes an outstanding
//! request. Keys are partitioned before these per-key steps by `key_matches`.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Once,
    Converges,
    Deduplicated,
    Approval,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upstream {
    pub copies: u64,
    pub incorrect: u64,
    pub outstanding: u64,
    pub repair_debt: u64,
    pub found: bool,
}

/// State-table :738-753. Request issuance is separate from application and reply.
/// Bound: counters fit u64; trace length and number of restarts are unbounded.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires (before.outstanding as int) < u64::MAX,
    ensures out.outstanding == (before.outstanding as int) + 1,
        out.copies == (before.copies as int), out.incorrect == (before.incorrect as int),
        out.repair_debt == (before.repair_debt as int), out.found == before.found,
))]
pub fn issue(before: Upstream) -> Upstream {
    Upstream {
        outstanding: before.outstanding + 1,
        ..before
    }
}

/// State-table Once: this is the REQUIRED UPSTREAM key-deduplication law, not a
/// claim about a local JSON receipt. PB publish reuses its sealed action key
/// (prismabuild/src/submit.rs:310-319); supervisor start reuses its operation ID.
/// Adapters' external implementations must satisfy this law for Once to hold.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires (before.outstanding as int) > 0, (before.copies as int) < u64::MAX,
        (before.incorrect as int) <= (before.copies as int),
    ensures out.outstanding == (before.outstanding as int) - 1,
        out.copies == if matches!(class, Class::Once | Class::Converges) { 1 }
            else { (before.copies as int) + 1 },
        out.incorrect == if class == Class::Approval && !correct {(before.incorrect as int) + 1}
            else if matches!(class, Class::Once | Class::Converges) {0}
            else {(before.incorrect as int)},
        out.repair_debt == (before.repair_debt as int), out.found == before.found,
))]
pub fn land(before: Upstream, class: Class, correct: bool) -> Upstream {
    let copies = match class {
        Class::Once | Class::Converges => 1,
        _ => before.copies + 1,
    };
    let incorrect = match class {
        Class::Once | Class::Converges => 0,
        Class::Approval if !correct => before.incorrect + 1,
        _ => before.incorrect,
    };
    Upstream {
        copies,
        incorrect,
        outstanding: before.outstanding - 1,
        ..before
    }
}

/// State-table :753: a timeout/lost response leaves future application enabled.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == before))]
pub fn lose_response(before: Upstream) -> Upstream {
    before
}

/// State-table reconciliation / Q27: list is a separate sample. Approval repairs
/// each noncanonical visible copy; messages delete all listed copies but the first.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires (before.incorrect as int) <= (before.copies as int),
    ensures out.copies == (before.copies as int), out.incorrect == (before.incorrect as int),
        out.outstanding == (before.outstanding as int), out.found == ((before.copies as int) > 0),
        out.repair_debt == if class == Class::Approval {(before.incorrect as int)}
            else if class == Class::Deduplicated && (before.copies as int) > 0 {(before.copies as int)-1}
            else {0},
))]
pub fn read(before: Upstream, class: Class) -> Upstream {
    let repair_debt = match class {
        Class::Approval => before.incorrect,
        Class::Deduplicated if before.copies > 0 => before.copies - 1,
        _ => 0,
    };
    Upstream {
        repair_debt,
        found: before.copies > 0,
        ..before
    }
}

/// State-table messages / Q27: one successful upstream DELETE or PATCH. A crash
/// can occur after this write and before any local receipt. A restart rereads.
/// HTTP failures with unknown application are represented by delaying this step.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires (before.repair_debt as int) > 0,
        class == Class::Approval ==> (before.incorrect as int) >= (before.repair_debt as int),
        class == Class::Deduplicated ==> (before.copies as int) > (before.repair_debt as int),
    ensures out.outstanding == (before.outstanding as int), out.found == before.found,
        out.repair_debt == (before.repair_debt as int)-1,
        out.copies == if class == Class::Deduplicated {(before.copies as int)-1} else {(before.copies as int)},
        out.incorrect == if class == Class::Approval {(before.incorrect as int)-1} else {(before.incorrect as int)},
))]
pub fn repair_one(before: Upstream, class: Class) -> Upstream {
    let copies = if class == Class::Deduplicated {
        before.copies - 1
    } else {
        before.copies
    };
    let incorrect = if class == Class::Approval {
        before.incorrect - 1
    } else {
        before.incorrect
    };
    Upstream {
        copies,
        incorrect,
        repair_debt: before.repair_debt - 1,
        ..before
    }
}

/// State-table :753 / Q27: crash/restart drops the list cursor, not upstream
/// writes or outstanding requests. A zombie can land before or after this step.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.copies == (before.copies as int), out.incorrect == (before.incorrect as int),
    out.outstanding == (before.outstanding as int), out.repair_debt == 0, out.found == false,
))]
pub fn restart(before: Upstream) -> Upstream {
    Upstream {
        repair_debt: 0,
        found: false,
        ..before
    }
}

/// State-table :753: a request may never apply. This is the upstream's eventual
/// nonapplication, not a timeout and not an executor's guess from an empty GET.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires (before.outstanding as int) > 0,
    ensures out.outstanding == (before.outstanding as int)-1,
        out.copies == (before.copies as int), out.incorrect == (before.incorrect as int),
        out.repair_debt == (before.repair_debt as int), out.found == before.found,
))]
pub fn never_lands(before: Upstream) -> Upstream {
    Upstream {
        outstanding: before.outstanding - 1,
        ..before
    }
}
