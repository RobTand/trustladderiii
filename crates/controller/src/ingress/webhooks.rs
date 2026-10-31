use super::*;

/// Receipt starts at bind, before controller recovery or source registration.
pub struct Webhooks {
    address: SocketAddr,
    control: WebhookControl,
    receiver: mpsc::Receiver<Input>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl Webhooks {
    pub fn bind(
        address: SocketAddr,
        store: Store,
        bootstrap: Bootstrap,
        secret: Vec<u8>,
        catch_up: Option<CatchUp>,
    ) -> Result<Self> {
        let source = WebhookReceiver::bind(address, store, bootstrap, secret, catch_up)?;
        let address = source.address();
        let control = source.control();
        let (sender, receiver) = mpsc::channel();
        let thread = Some(thread::spawn(move || source.run(sender)));
        Ok(Self {
            address,
            control,
            receiver,
            thread,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn control(&self) -> WebhookControl {
        self.control.clone()
    }
}

impl Source for Webhooks {
    fn run(mut self, sender: Sender<Input>) -> Result<()> {
        self.control.gap();
        for input in &self.receiver {
            let _ = sender.send(input);
        }
        self.thread
            .take()
            .expect("webhook receiver thread")
            .join()
            .map_err(|_| Error::Invalid("webhook receiver panicked"))?
    }
}

impl Drop for Webhooks {
    fn drop(&mut self) {
        self.control.stop();
    }
}

pub(super) fn fetch_catch_up(
    catch_up: CatchUp,
    mut reader: tl3_store::Reader,
    bootstrap: Bootstrap,
    sender: Sender<WebhookMessage>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (cursor, snapshot) = catch_up_state(&mut reader).or_else(|_| {
                reader.reconnect()?;
                catch_up_state(&mut reader)
            })?;
            let since = cursor
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(DateTime::UNIX_EPOCH);
            catch_up.fetch(since, snapshot, &bootstrap)
        }))
        .unwrap_or(Err(Error::Invalid("GitHub catch-up panicked")));
        // Only the receipt loop inserts rows, so sequence order remains commit order.
        let _ = sender.send(WebhookMessage::CaughtUp(Box::new(reader), result));
    })
}

fn catch_up_state(
    reader: &mut tl3_store::Reader,
) -> tl3_store::Result<(Option<serde_json::Value>, tl3_store::Snapshot)> {
    Ok((reader.ingress_cursor("github-catch-up")?, reader.load()?))
}

#[cfg(test)]
trait Fetcher: Send + Sync {
    fn fetch(
        &self,
        since: DateTime<Utc>,
        snapshot: tl3_store::Snapshot,
        bootstrap: &Bootstrap,
    ) -> Result<CatchUpBatch>;
}

#[cfg(test)]
impl Webhooks {
    fn bind_with_fetcher(
        _store: Store,
        _fetcher: Arc<dyn Fetcher>,
    ) -> Result<Self> {
        todo!("fakeable fetch boundary for the real receipt loop")
    }
}

#[cfg(test)]
mod tests;
