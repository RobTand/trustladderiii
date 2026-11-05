//! K-required-check: port of base 504f10b0 enqueue_proofs.rs:311-356.
//! Q47 (docs/state-machine.md:681-688), state-table.md:540-541,549,575
//! distinguish code and infrastructure failures. Q52 (state-machine.md:712-722,
//! state-table.md:542-544) concerns absent required checks, handled elsewhere.
//! #447 and state-table.md:545,578 concern blocked queue incidents; #608 records
//! served incident RobTand/prismabuild#2705, whose public-view PR is not a
//! classifier rule. The exact enum partition, including unknown conclusions,
//! is preserved existing behaviour (no rule). The full-domain contract is
//! inline on the real required_check body; covers are in the witness module.
//! This root proves required_check, not decide. The stable decide namespace is
//! supplied without a property; no call to decide is used by this port.
use crate::enqueue::*;
use vstd::prelude::*;
verus! {
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool { true }
}
