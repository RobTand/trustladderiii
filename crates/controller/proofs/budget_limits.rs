#[path = "../src/budget/limits.rs"]
mod limits;
use limits::*;
use vstd::prelude::*;
verus! {
/// Q4/#743: fit is downward closed on the original full-width ordered domain.
fn fit_monotone(small: u64, big: u64, limit: u64)
    requires small <= big,
{
    let bigger = fits(big, limit);
    let smaller = fits(small, limit);
    assert(!bigger || smaller);
    assert(smaller == (small < limit));
}
/// Q4/#743: executable covers call the same real fit decision in both regions.
fn fit_witness(case: bool) -> (result: bool)
    ensures result,
{
    let small = if case { 0 } else { 1 };
    let big = small;
    let limit = 1;
    let actual = fits(big, limit);
    let smaller = fits(small, limit);
    assert(!actual || smaller);
    assert(smaller == (small < limit));
    if case { actual } else { !smaller }
}
/// #698/#718: original room-remains and overspent covers execute remaining.
fn remaining_witness(case: bool) -> (result: bool)
    ensures result,
{
    let used = if case { 0 } else { 2 };
    let allowed = 1;
    let room = remaining(used, allowed);
    if case { used < allowed && room > 0 } else { used > allowed && room == 0 }
}
/// #698: original strict-reduction and cannot-reduce compaction covers.
fn compaction_witness(case: bool) -> (result: bool)
    ensures result,
{
    let actual = compacted(1, if case { 0 } else { 1 });
    if case { actual } else { !actual }
}
/// #692/#718: original verification-reserve and maker-working-room covers.
fn reserve_witness(case: bool) -> (result: bool)
    ensures result,
{
    let maker = true;
    let actual = finalizing(maker, if case { 1 } else { 0 }, 0, 1, 0, 0);
    if case { actual } else { maker && !actual }
}
/// #698/#718: construct each of the six original exhaustion outcomes.
fn exhaustion_witness(case: u8) -> (result: bool)
    requires case < 6,
    ensures result,
{
    let r = Run { maker: case == 0, turns: if case < 2 { 1 } else { 0 },
        allowed: 1, stalled: case == 2, context_full: case == 3,
        spent: if case == 4 { 1 } else { 0 }, predicted: 0, reserve: 0, input_limit: 0 };
    let actual = exhaustion(r);
    actual == match case {
        0 => Some(Exhaustion::Backstop), 1 => Some(Exhaustion::ToolTurns),
        2 => Some(Exhaustion::NoProgress), 3 => Some(Exhaustion::ContextTokens),
        4 => Some(Exhaustion::InputTokens), _ => None,
    }
}
}
fn main() {}
