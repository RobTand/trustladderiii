use super::decision::*;

/// Q4 / state-table "Fit and splits": increasing size cannot restore fit.
#[kani::proof]
#[kani::unwind(1)] // No loops; arbitrary full-width sizes, not a sampled bound.
fn fit_monotone() {
    let small:u64=kani::any(); let big:u64=kani::any(); let limit:u64=kani::any();
    kani::assume(small<=big);
    assert!(!fits(big,limit)||fits(small,limit));
    assert_eq!(fits(small,limit),small<limit);
    kani::cover!(fits(big,limit),"fits");
    kani::cover!(!fits(small,limit),"does not fit");
}

/// #698/#718: remaining allowance is bounded, including exhausted/recovered runs.
#[kani::proof]
#[kani::unwind(1)] // No loops; exhausts both u64 counters including overflow edges.
fn budget_bounds() {
    let used:u64=kani::any(); let allowed:u64=kani::any();
    let room=remaining(used,allowed);
    assert!(room<=allowed);
    if used<=allowed {assert_eq!(u128::from(room)+u128::from(used),u128::from(allowed));}
    else {assert_eq!(room,0);}
    kani::cover!(used<allowed && room>0,"room remains");
    kani::cover!(used>allowed && room==0,"over budget");
}

/// Q4 / #718: the 85% boundary is exact, with no multiplication overflow.
#[kani::proof]
#[kani::unwind(1)] // No loops; full u64 domain, wide arithmetic in the oracle.
fn cutoff_exact() {
    let tokens:u64=kani::any(); let size:u64=kani::any();
    let actual=cutoff(tokens,size);
    // The equivalent 20/17 specification is proved by Verus. Kani checks
    // this full-domain product comparison and its overflow obligations.
    assert_eq!(actual,u128::from(tokens)*100>=u128::from(size)*85);
    kani::cover!(actual,"out of room");
    kani::cover!(!actual,"room remains");
}

/// #698: effective compaction strictly reduces size; all other results say cannot.
#[kani::proof]
#[kani::unwind(1)] // No loops; every pair of u64 sizes, including an empty context.
fn compaction_reduces() {
    let before:u64=kani::any(); let after:u64=kani::any();
    assert_eq!(compacted(before,after),before>after);
    kani::cover!(compacted(before,after),"strict reduction");
    kani::cover!(!compacted(before,after),"cannot reduce");
}

/// #718 / #692: reserve cannot buy edits, even after restoring at the boundary.
#[kani::proof]
#[kani::unwind(1)] // No loops; reserve is capped to the full u64 tool budget.
fn reserve_preserved() {
    let maker:bool=kani::any(); let turns:u64=kani::any(); let no_progress:u64=kani::any();
    let allowed:u64=kani::any(); let window:u64=kani::any(); let reserve:u64=kani::any();
    let actual=finalizing(maker,turns,no_progress,allowed,window,reserve);
    let reserve=u128::from(reserve.min(allowed));
    assert_eq!(actual,maker && (u128::from(turns)+reserve>=u128::from(allowed)
        || (window!=0 && u128::from(no_progress)+reserve>=u128::from(window))));
    kani::cover!(actual,"verification reserve");
    kani::cover!(maker && !actual,"working room");
}

/// #698/#718: backstop, no-progress, context and input stops keep their precedence.
#[kani::proof]
#[kani::unwind(1)] // No loops; finite flags and arbitrary full-width counters.
fn exhaustion_exact() {
    let r:Run=kani::any();
    let expected=if r.turns>=r.allowed {Some(if r.maker{Exhaustion::Backstop}else{Exhaustion::ToolTurns})}
        else if r.stalled {Some(Exhaustion::NoProgress)}
        else if r.context_full {Some(Exhaustion::ContextTokens)}
        else if u128::from(r.spent)+u128::from(r.predicted)+u128::from(r.reserve)>u128::from(r.input_limit) {Some(Exhaustion::InputTokens)}
        else {None};
    let actual=exhaustion(r);
    assert_eq!(actual,expected);
    kani::cover!(actual==Some(Exhaustion::Backstop),"backstop");
    kani::cover!(actual==Some(Exhaustion::ToolTurns),"tool turns");
    kani::cover!(actual==Some(Exhaustion::NoProgress),"no progress");
    kani::cover!(actual==Some(Exhaustion::ContextTokens),"context");
    kani::cover!(actual==Some(Exhaustion::InputTokens),"input allowance");
    kani::cover!(actual.is_none(),"continue");
}
