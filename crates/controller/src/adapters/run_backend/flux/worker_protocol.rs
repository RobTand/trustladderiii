//! FX3-owned worker transport collector. Shared API frozen by FX1.
use super::*;

// The source boundary is published before FX3 implements the protocol parsers.
// Until then these paths explicitly remain unqualified.
pub(super) fn report_identity(_record: &Record, _report: &RunnerReport) -> Result<bool> {
    Ok(false)
}

impl Flux {
    pub fn collect_worker(
        &self,
        _owner: &Owner,
        _input: &CollectorInput,
    ) -> Result<WorkerCollection> {
        Err(BackendError {
            code: ErrorCode::Unavailable,
            message: "worker protocol collector is not qualified yet".into(),
        })
    }

    pub(super) fn verified_worker_payload(
        &self,
        owner: &Owner,
        terminal: &Terminal,
    ) -> Result<WorkerPayload> {
        let report = self.verified_report(owner, terminal)?;
        report
            .worker_payload
            .ok_or_else(|| BackendError::invalid("verified worker payload absent"))
    }
}
