//! #806 / Q18: compose the verified credit handler with a late merge's exact result.
use super::*;

verus! {
/// #806 / Q18: a late Stopped/Replaced merge corrects the outcome and credits once.
#[verifier::spinoff_prover]
pub(super) proof fn late_result(before: Node, after: Node, head: HeadId,
    checks_passed: bool)
    requires before.had_pr, before.merged.is_none(),
        matches!(before.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced)),
        before.credit.merge_credited || before.credit.earned < u64::MAX,
        after == (Node { merged: Some(head), phase: Phase::Closed(Outcome::Merged),
            credit: Credit { merge_credited: true,
                earned: if before.credit.merge_credited { before.credit.earned }
                    else { (before.credit.earned + 1) as u64 }, ..before.credit }, ..before }),
    ensures after == retirement::fact_node(before, Fact::PrMerged { head, checks_passed }),
        settle_shape(before) ==> settle_shape(after),
        after.credit.earned as int + merge_capacity(after) <= before.credit.earned as int + 1,
{
    reveal(retirement::fact_node);
}
}

verus! {
/// #806 merge rows: duplicate, unrelated and final non-correctable merges are inert.
#[verifier::spinoff_prover]
pub(super) proof fn ignored_result(before: Node, head: HeadId, checks_passed: bool)
    requires !before.had_pr || before.merged.is_some()
        || (before.phase is Closed && !matches!(before.phase,
            Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced))),
    ensures retirement::fact_node(before, Fact::PrMerged { head, checks_passed }) == before,
{
    reveal(retirement::fact_node);
}

/// #806 / Q18: the verified open-merge handler supplies the exact fact and credit bound.
#[verifier::spinoff_prover]
pub(super) proof fn open_result(before: Node, after: Node, head: HeadId,
    checks_passed: bool)
    requires before.had_pr, before.merged.is_none(), !(before.phase is Closed),
        after == retirement::stopped(Node { merged: Some(head), pr_open: false, ..before },
            Outcome::Merged),
        after.credit == before.credit,
    ensures after == retirement::fact_node(before, Fact::PrMerged { head, checks_passed }),
        after.credit.earned as int + merge_capacity(after) <= before.credit.earned as int + 1,
{
    reveal(retirement::fact_node);
}
}
