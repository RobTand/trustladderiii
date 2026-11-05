#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan { pub keep: Option<usize>, pub delete_from: usize }
pub fn plan(_copies: usize) -> Plan { Plan { keep: None, delete_from: 0 } }
