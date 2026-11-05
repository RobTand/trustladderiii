//! #823: abstract document reduction reference, executed by the conformance test.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub kind: u8,
    pub removed: usize,
    pub bytes: u64,
}

pub fn reduce(_full: u64, _core: u64, _diagnostic: u64, _prefix_sizes: &[u64], _limit: u64) -> Plan {
    todo!("verified reduction reference")
}
