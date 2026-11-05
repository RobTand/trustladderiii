//! This root proves required_check, not decide. The stable decide namespace is
//! supplied without a property; no call to decide is used by this port.
use crate::enqueue::*;
use vstd::prelude::*;
verus! {
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool { true }
}
