mod decision;
use vstd::prelude::*;
fn main() {}
verus! {
/// Q54: both self-check and finding maps use the same real coverage function.
/// Unbounded counts; same_ids is the parser's exact BTreeSet equality fact.
fn reproductions_are_one_to_one(expected: u64, actual: u64, same_ids: bool) {
    let covers = decision::exact_coverage(expected, actual, same_ids);
    assert(covers <==> (expected == actual && same_ids));
}
/// Q54: witnesses reach acceptance and each rejection predicate, avoiding vacuity.
/// All Boolean stage combinations and u64 counts, without a sequence bound.
fn coverage_and_stage_witnesses() {
    let valid = decision::exact_coverage(2, 2, true);
    let short = decision::exact_coverage(2, 1, true);
    let different = decision::exact_coverage(2, 2, false);
    assert(valid && !short && !different);
    let maker_ok = decision::stage_valid(true, true, false);
    let maker_bad = decision::stage_valid(true, false, true);
    let downstream_ok = decision::stage_valid(false, false, true);
    let downstream_bad = decision::stage_valid(false, true, false);
    assert(maker_ok && !maker_bad && downstream_ok && !downstream_bad);
}
}
