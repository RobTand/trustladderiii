//! #806 standing rule: enter_stopping copied verbatim from a6a9d96ad before extraction.
use super::*;

kernel_fn! {
/// Stop and replacement rows / #771: at most one generation is allocated.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn reference_enter_stopping(m: &mut Node, outcome: Outcome) -> (result: ()) [
    requires !(old(m).phase is Closed || old(m).phase is Stopping)
        ==> old(m).generation < u64::MAX,
    ensures *final(m) == retirement::stopped(*old(m), outcome),
        (!old(m).paused && !old(m).waiting_incident && old(m).effect.is_none()
            && !(old(m).phase is Closed || old(m).phase is Stopping)
            ==> distance(*final(m)) < distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int == old(m).generation as int
            + if old(m).phase is Closed || old(m).phase is Stopping { 0int } else { 1int },
        final(m).phase == match old(m).phase {
            Phase::Closed(o) => Phase::Closed(o),
            Phase::Stopping(o) => Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }),
            _ => Phase::Stopping(outcome),
        },
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd,
        final(m).review_stop == old(m).review_stop,
        (old(m).phase is Closed || old(m).phase is Stopping) ==> *final(m) == (Node {
            phase: final(m).phase, ..*old(m)
        }),
        !(old(m).phase is Closed || old(m).phase is Stopping) ==>
            final(m).pos == 0 && final(m).ticket.is_none() && final(m).question.is_none()
                && !final(m).waiting_incident && !final(m).incident_needs_rob,
] proof [reveal(retirement::stopped);] {
    kernel_snapshot! { before = *m }
    kernel_proof! {
        allocation_stopping_lemmas::stopping_frame(before@, retirement::stopped(before@, outcome), outcome);
        allocation_stopping_lemmas::stopping_progress(before@, retirement::stopped(before@, outcome), outcome);
    }
    match m.phase {
        Phase::Closed(_) => {}
        Phase::Stopping(_) => {
            if outcome == Outcome::Merged {
                m.phase = Phase::Stopping(Outcome::Merged);
            }
        }
        _ => {
            m.question = None;
            m.waiting_incident = false;
            m.incident_needs_rob = false;
            set_phase(m, Phase::Stopping(outcome));
        }
    }
}
}
