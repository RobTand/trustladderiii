use crate::translate_arms::*;
use vstd::prelude::*;
verus! {
/// Reply rule; docs/state-table.md:139, c3e6fbbb. Two witnesses cover zero and
/// maximal generation; the production proof has no finite generation bound.
pub fn accepts_current_rebase() {
    let zero = rebase_reply(RebasePhase::Rebasing, 0u64, 0u64);
    let maximum = rebase_reply(RebasePhase::Rebasing, u64::MAX, u64::MAX);
    assert(zero == RebaseDisposition::Accept);
    assert(maximum == RebaseDisposition::Accept);
}
/// Reply rule; docs/state-table.md:139, c3e6fbbb. An equal-generation reply in
/// another phase is ignored before conflict rows or head interning.
pub fn rejects_other_phase() {
    let reply = rebase_reply(RebasePhase::Other, 7u64, 7u64);
    assert(reply == RebaseDisposition::Ignore);
}
/// Reply rule; docs/state-table.md:139, c3e6fbbb. Both older and newer replies
/// are ignored. These two witnesses supplement the universal equality proof.
pub fn rejects_other_generation() {
    let older = rebase_reply(RebasePhase::Rebasing, 7u64, 6u64);
    let newer = rebase_reply(RebasePhase::Rebasing, 7u64, 8u64);
    assert(older == RebaseDisposition::Ignore);
    assert(newer == RebaseDisposition::Ignore);
}
}
