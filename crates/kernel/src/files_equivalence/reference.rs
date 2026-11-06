//! Verbatim pre-extraction reply_files executable body (main 3587a599 / W1 beb05482), test-only.
use super::*;

pub(super) fn reference(w: &mut World, i: usize, files: Footprint, fits: bool, out: &mut Outbox) {
    if fits && !files.is_empty() && w.nodes[i].credit.can_spend(1) {
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        let m = &mut w.nodes[i];
        m.credit.spend(1);
        m.footprint = footprint;
        m.holds = false;
        set_phase(m, Phase::Ready(Resume::Making));
    } else if fits && !files.is_empty() && w.fail_fast {
        // Q54 stops retries, not credit decisions: Rob funds the extension.
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        w.nodes[i].footprint = footprint;
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(Resume::Making),
            1,
            out,
        );
    } else {
        out_of_room(w, i, ReviewStop::MakerNeedsFiles, out);
    }
}
