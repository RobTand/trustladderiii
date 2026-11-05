trait EvidenceExt {
    fn kind(&self) -> tl3_mergify::decide::translate::Kind;
}
impl EvidenceExt for adapters::mergify::Evidence {
    fn kind(&self) -> tl3_mergify::decide::translate::Kind {
        use adapters::mergify::Evidence as E;
        use tl3_mergify::decide::translate::Kind as K;
        match self {
            E::ChecksUnavailable { .. } => K::ChecksUnavailable,
            E::CheckMissing { .. } => K::CheckMissing,
            E::QueueBlocked { .. } => K::QueueBlocked,
            E::CheckReported { .. } => K::CheckReported,
            E::Merged { checks_passed, .. } => K::Merged {
                checks_passed: *checks_passed,
            },
            E::CiFailed {
                head_check, infra, ..
            } => K::CiFailed {
                head_check: *head_check,
                infra: *infra,
            },
            E::Conflict { .. } => K::Conflict,
            E::DequeuedByCommand { .. } => K::DequeuedByCommand,
            E::DequeuedByPerson { .. } => K::DequeuedByPerson,
            E::ClosedByPerson { .. } => K::ClosedByPerson,
            E::PushByOther { .. } => K::PushByOther,
        }
    }
}
