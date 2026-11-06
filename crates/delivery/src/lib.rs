#![no_std]
#![forbid(unsafe_code)]

mod step;
pub use step::*;

pub mod mirror;
pub mod upstream;

#[cfg(test)]
mod round2 {
    use super::*;
    use mirror::*;
    use upstream::*;

    #[test]
    fn history_found_and_pending_commit_sent_first() {
        for read in [Read::Found, Read::Pending] {
            assert_eq!(approval_history(true, false, false, read).sent, State::Sent);
        }
    }
    #[test]
    fn retryable_kernel_old_key_is_cancelled() {
        assert_eq!(
            consumed_completion(State::Uncertain, Ack::Retryable, true),
            State::Cancelled
        );
    }
    #[test]
    fn human_clear_and_sent_consume_the_grant_atomically() {
        let ambiguous = Approval {
            state: State::Uncertain,
            grant: Grant::None,
            lineage_safe: false,
            unresolved: true,
            incident_open: true,
            repair: false,
        };
        let granted = clear_approval(ambiguous);
        assert_eq!(granted.state, State::Intended);
        assert_eq!(granted.grant, Grant::HumanClear);
        assert!(granted.lineage_safe);
        let sent = send_approval(granted);
        assert_eq!(sent.state, State::Sent);
        assert_eq!(sent.grant, Grant::None);
    }
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
