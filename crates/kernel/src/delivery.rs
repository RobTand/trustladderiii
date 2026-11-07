//! Accepted-delivery entry. PRD #878, approved S3 design, entry row.
use super::*;

kernel_fn! {
    /// Enqueue accepted: release ownership with the single phase generation.
    pub(super) fn accept(m: &mut Node) -> (result: ()) [
        requires old(m).generation < u64::MAX, old(m).effect.is_none(),
        ensures final(m).phase == Phase::AwaitingDelivery,
            final(m).generation as int == old(m).generation as int + 1,
            final(m).pos == 0, final(m).ticket.is_none(), !final(m).holds,
            final(m).effect.is_none(),
            final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
            final(m).head == old(m).head, final(m).checked == old(m).checked,
            final(m).approved == old(m).approved, final(m).credit == old(m).credit,
            final(m).fx_failures == 0, final(m).fx_incidents == 0,
    ] {
        m.fx_failures = 0;
        m.fx_incidents = 0;
        m.holds = false;
        set_phase(m, Phase::AwaitingDelivery);
    }
}
