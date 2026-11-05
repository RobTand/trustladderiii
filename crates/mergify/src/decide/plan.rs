//! Execute-shell plans. Sampling and effects remain in the adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentEffect {
    Save,
    Fence,
}
pub fn fenced(_incoming: i64, _current: i64) -> bool {
    todo!("A2 epoch decision")
}
pub fn intent() -> (IntentEffect, IntentEffect) {
    todo!("A2 write-ahead plan")
}
