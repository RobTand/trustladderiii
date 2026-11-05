#[path = "../src/paths/decision.rs"]
mod decision;
use vstd::prelude::*;
verus! {
/// #820 protected paths: star cannot cross `/`, while recursive star can.
/// Every Boolean state of the production NFA transition, without a length bound.
fn star_separator(same: bool, diagonal: bool, left: bool, older: bool, accepts: bool) {
    let star=decision::transition(decision::Step::Star,same,diagonal,left,older,true,accepts);
    assert(star==same);
    let recursive=decision::transition(decision::Step::Recursive,same,diagonal,left,older,true,accepts);
    assert(recursive==(same||left));
}
/// #820 recursive `**/` includes zero directories and any nonempty slash-ended prefix.
fn directories(same: bool, older: bool, separator: bool) {
    let result=decision::transition(decision::Step::Directories,same,false,false,older,separator,false);
    assert(result==(same||older&&separator));
}
/// #820 every AST transition has reachable accept and reject states.
fn witnesses() {
    let literal=decision::transition(decision::Step::Character,false,true,false,false,false,true); assert(literal);
    let rejected=decision::transition(decision::Step::Character,false,true,false,false,false,false); assert(!rejected);
    let empty=decision::transition(decision::Step::Star,true,false,false,false,false,false); assert(empty);
    let slash=decision::transition(decision::Step::Star,false,true,true,true,true,true); assert(!slash);
    let plus=decision::transition(decision::Step::NonemptyStar,false,true,false,false,false,false); assert(plus);
    let recursive=decision::transition(decision::Step::Recursive,false,false,true,false,true,false); assert(recursive);
    let root=decision::transition(decision::Step::Directories,true,false,false,false,false,false); assert(root);
    let nested=decision::transition(decision::Step::Directories,false,false,false,true,true,false); assert(nested);
}
}
fn main() {}
