// Function bodies are checked against their pinned source by the harness.
use super::io::*;
use super::*;
impl WorkerOutputSource {
    fn refresh(
        &mut self,
        _run: &mut Run,
        _store: &mut Store,
        _sender: &Sender<Input>,
    ) -> Result<bool> {
        todo!("F1 extracted refresh")
    }
}
pub(super) fn parse_reply(
    _bytes: &[u8],
    _delivery: &Delivery,
    _inputs: &Value,
) -> Result<Observation> {
    todo!("F1 typed event sequence")
}

pub(super) fn refresh_run(source: &mut WorkerOutputSource, run: &mut Run) -> Result<bool> {
    source.refresh(run, &mut Store, &Sender(std::marker::PhantomData))
}
