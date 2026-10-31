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
            if sender.send(input).is_err() {
                self.control.stop();
                break;
            }
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
    store: &mut Store,
    bootstrap: Bootstrap,
    sender: Sender<WebhookMessage>,
) -> Result<thread::JoinHandle<()>> {
    let since = store
        .ingress_cursor("github-catch-up")?
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or(DateTime::UNIX_EPOCH);
    let snapshot = store.load()?;
    Ok(thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            catch_up.fetch(since, snapshot, &bootstrap)
        }))
        .unwrap_or(Err(Error::Invalid("GitHub catch-up panicked")));
        // Only the receipt loop inserts rows, so sequence order remains commit order.
        let _ = sender.send(WebhookMessage::CaughtUp(result));
    }))
}
