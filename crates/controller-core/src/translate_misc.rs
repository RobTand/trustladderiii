//! Pure residue of the miscellaneous observation arms. I/O stays in translate.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Escape: docs/state-table.md:145 and Q15 in docs/state-machine.md:477 at
/// 9a658a8e are comment-classification context. This exact intake gate preserves
/// existing behaviour (no rule): controller/src/lib.rs:3042-3043 on that base.
/// The Boolean domain is exhaustive; there are no loops or recursion.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (author_matches && repository_configured),
))]
pub fn admit_comment(author_matches: bool, repository_configured: bool) -> bool {
    author_matches && repository_configured
}

/// Escape: preserved existing behaviour (no rule), controller/src/lib.rs:3052-3056
/// at 9a658a8e; ab534876a2 introduced retirement/snapshot deduplication.
/// The shell reads comment_retired before sampling the snapshot. This decision
/// cannot move or repeat the read. No loop, recursion or sequence bound is used.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (!retired && !recorded),
))]
pub fn record_comment(retired: bool, recorded: bool) -> bool {
    !retired && !recorded
}
