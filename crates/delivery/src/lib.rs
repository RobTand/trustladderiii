#![no_std]
#![forbid(unsafe_code)]

mod step;
pub use step::*;

pub mod upstream;

#[cfg(test)]
mod round2 {
    use crate::upstream::*;

    #[test]
    fn issued_patch_and_delete_land_after_restart_idempotently() {
        for kind in [RepairKind::Delete, RepairKind::Patch] {
            let copy = CopyState {
                id: 9,
                exists: true,
                conclusion: 0,
                output: 0,
            };
            let request = RepairRequest {
                target: 9,
                kind,
                conclusion: 1,
                output: 2,
            };
            let flight = RepairFlight {
                request,
                issued: true,
                cursor: true,
            };
            let restarted = restart_repair(flight);
            let repaired = apply_repair(copy, restarted);
            match kind {
                RepairKind::Delete => assert!(!repaired.exists),
                RepairKind::Patch => assert_eq!((repaired.conclusion, repaired.output), (1, 2)),
            }
            assert_eq!(apply_repair(repaired, restarted), repaired);
            assert_eq!(
                apply_repair(CopyState { id: 10, ..copy }, restarted),
                CopyState { id: 10, ..copy }
            );
        }
    }
}

#[cfg(test)]
mod action_tests;
