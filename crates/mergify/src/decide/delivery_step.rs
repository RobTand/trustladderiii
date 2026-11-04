//! Minimal acknowledgement reference model for the effect machine's final states.
//!
//! This is a toolchain qualification model, not a proof of controller delivery,
//! HTTP ordering, persistence, fences, or exactly-once upstream effects.

/// State-table "Effects": a final acknowledgement cannot become pending again.
/// This step models only receipt accumulation, with no I/O or scheduling claims.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(next => ensures
        next == (delivered || acknowledgement),
        delivered ==> next,
    )
)]
pub fn step(delivered: bool, acknowledgement: bool) -> bool {
    delivered || acknowledgement
}

#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_acknowledgement_stays_delivered() {
        assert!(super::step(super::step(false, true), true));
    }
}
