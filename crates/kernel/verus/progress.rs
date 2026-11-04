//! #771 source-connected proof root. Counter and progress contracts are added here.
extern crate alloc;
use vstd::prelude::*;
#[path = "../src/lib.rs"]
mod kernel;
pub use kernel::*;

verus! {
/// #771: a fresh u64 command ID has one unit of arithmetic headroom.
proof fn command_headroom(last: u64)
    requires last < u64::MAX,
    ensures last as int + 1 <= u64::MAX as int,
{
}
}
fn main() {}
