//! Body-heal decisions over parsed upstream inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub id: u64,
    pub current_head: bool,
    pub pull_request: bool,
    pub listed: bool,
    pub completed: bool,
    pub failed: bool,
}
pub fn available(_open: bool, _merged: bool) -> bool {
    todo!("A2 heal scope")
}
pub fn newest(_runs: &[Run]) -> Option<usize> {
    todo!("A2 workflow selection")
}
