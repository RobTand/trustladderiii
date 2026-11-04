//! Test-only, per-thread allocation accounting for the complete #783 path.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Instant,
};
thread_local! { static COUNTS: Cell<Option<(u64,u64)>> = const {Cell::new(None)}; }
struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn allocated(bytes: usize) {
    let _ = COUNTS.try_with(|counts| {
        if let Some((n, b)) = counts.get() {
            counts.set(Some((n + 1, b + bytes as u64)));
        }
    });
}
// Safety: all pointers, sizes and layouts pass unchanged to System. Only this
// thread's allocation count and requested bytes are observed; nothing is retained.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocated(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocated(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
pub(crate) fn measure(run: impl FnOnce()) -> (u128, u64, u64) {
    COUNTS.with(|c| c.set(Some((0, 0))));
    let start = Instant::now();
    run();
    let ns = start.elapsed().as_nanos();
    let (allocs, bytes) = COUNTS.with(|c| c.replace(None).unwrap());
    (ns, allocs, bytes)
}
