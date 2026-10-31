use super::*;
use crate::sync::Mutex;

/// Receipt starts at bind, before controller recovery or source registration.
pub struct Webhooks {
    address: SocketAddr,
    control: WebhookControl,
    notifications: Arc<Notifications>,
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
        Ok(Self::start(source))
    }

    fn start(source: WebhookReceiver) -> Self {
        let address = source.address();
        let control = source.control();
        let notifications = Arc::new(Notifications::new());
        let receipts = Arc::clone(&notifications);
        let thread = Some(thread::spawn(move || source.run(receipts)));
        Self {
            address,
            control,
            notifications,
            thread,
        }
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
        self.notifications.register(sender);
        self.control.gap();
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
    catch_up: Arc<dyn Fetcher>,
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

pub(super) trait Fetcher: Send + Sync {
    fn fetch(
        &self,
        since: DateTime<Utc>,
        snapshot: tl3_store::Snapshot,
        bootstrap: &Bootstrap,
    ) -> Result<CatchUpBatch>;
}

impl Fetcher for CatchUp {
    fn fetch(
        &self,
        since: DateTime<Utc>,
        snapshot: tl3_store::Snapshot,
        bootstrap: &Bootstrap,
    ) -> Result<CatchUpBatch> {
        CatchUp::fetch(self, since, snapshot, bootstrap)
    }
}

#[cfg(all(test, not(loom)))]
impl Webhooks {
    fn bind_with_fetcher(store: Store, fetcher: Arc<dyn Fetcher>) -> Result<Self> {
        let reader = store.reader()?;
        let mut source = WebhookReceiver::bind(
            "127.0.0.1:0".parse().unwrap(),
            store,
            tests::bootstrap(),
            b"test-secret".to_vec(),
            None,
        )?;
        source.catch_up = Some(fetcher);
        source.catch_up_reader = Some(reader);
        Ok(Self::start(source))
    }
}

pub(super) struct Notifications(Mutex<WakeTarget>);
enum WakeTarget {
    Pending(bool),
    Registered(Sender<Input>),
}
impl Notifications {
    pub(super) fn new() -> Self {
        Self(Mutex::new(WakeTarget::Pending(false)))
    }
    pub(super) fn wake(&self) {
        match &mut *self.0.lock().expect("webhook notifications") {
            WakeTarget::Pending(pending) => *pending = true,
            WakeTarget::Registered(sender) => {
                let _ = sender.send(Input::GitHubWake);
            }
        }
    }
    pub(super) fn register(&self, sender: Sender<Input>) {
        let mut target = self.0.lock().expect("webhook notifications");
        if matches!(*target, WakeTarget::Pending(true)) {
            let _ = sender.send(Input::GitHubWake);
        }
        *target = WakeTarget::Registered(sender);
    }
}

pub(super) enum FetchState {
    Idle,
    Running(thread::JoinHandle<()>),
    // Some: coalesced gaps during a fetch. None: a failed pass awaiting an event.
    Wanted(Option<thread::JoinHandle<()>>),
}
impl FetchState {
    pub(super) fn request(&mut self, start: impl FnOnce() -> thread::JoinHandle<()>) {
        *self = match std::mem::replace(self, Self::Idle) {
            Self::Idle | Self::Wanted(None) => Self::Running(start()),
            Self::Running(thread) | Self::Wanted(Some(thread)) => Self::Wanted(Some(thread)),
        };
    }
    pub(super) fn waiting(&self) -> bool {
        matches!(self, Self::Wanted(None))
    }
    pub(super) fn join(self) -> Result<()> {
        if let Self::Running(thread) | Self::Wanted(Some(thread)) = self {
            thread
                .join()
                .map_err(|_| Error::Invalid("GitHub catch-up panicked"))?;
        }
        Ok(())
    }
}

#[cfg(all(test, not(loom)))]
mod tests;

#[cfg(all(test, loom))]
#[path = "../../tests/loom/notifications.rs"]
mod loom_tests;
