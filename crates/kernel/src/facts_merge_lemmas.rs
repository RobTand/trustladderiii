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
