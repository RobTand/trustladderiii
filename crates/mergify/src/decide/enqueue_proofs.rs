//! Kani harnesses for `enqueue::decide`: all retired to Verus.
//!
//! The ctrl-core slices ported every harness and deleted it in the same commit as
//! its gated Verus replacement (#806 bar):
//! - B2: `enqueue_refused_sticky`, `enqueue_write_ahead`, `enqueue_recovery_grace`;
//! - A4: `enqueue_ack_sound`, `enqueue_stale_head`, `enqueue_dequeue_fresh`;
//! - C2: `enqueue_required_check`.
//!
//! The contracts and witnesses live in `crates/mergify/proofs/enqueue_*.rs`. Their
//! per-assertion equivalence maps are `enqueue_a4_equivalence.md` and the B2/C2
//! hand-backs. This module stays registered in `tools/kani.py` with an empty
//! inventory, where the scan, prove and mutant modes are successful no-ops.
