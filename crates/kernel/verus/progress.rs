//! #771 source-connected scoped safety proof root.
extern crate alloc;
use vstd::prelude::*;
#[path = "../src/lib.rs"]
mod kernel;
pub use kernel::*;
extern crate self as tl3_kernel;
#[path = "../../controller/src/kernel_boundary.rs"]
mod controller_boundary;

verus! {
/// State-table "Counter headroom": a fresh command ID has room for one increment.
proof fn command_headroom(last: u64)
    requires last < u64::MAX,
    ensures last as int + 1 <= u64::MAX as int,
{
}
}
fn main() {}
