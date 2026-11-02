//! The TL3 operator console (PRD #327): a read-only board, served apart from
//! tl3d. It reads Postgres through read-only sessions and never writes the
//! world snapshot or the event log.

#![forbid(unsafe_code)]

pub mod api;
mod feed;
mod rebuild;
mod server;
mod supervise;

use rebuild::{Change, Rebuilds};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use supervise::Supervisor;
use tl3_controller::board::{Board, Rows};
use tl3_store::Reader;
use tokio::sync::watch;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Config(#[from] tl3_config::Error),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("console I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("the console's {0} thread stopped; exiting so the service restarts")]
    Stopped(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Options {
    pub store: tl3_config::Store,
    /// The built web app.
    pub assets: PathBuf,
    /// Model ladders, to name each card's model.
    pub models: Option<tl3_config::Models>,
}

/// A console serving on its own threads.
pub struct Running {
    address: SocketAddr,
    supervisor: Supervisor,
}

impl Running {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Block while the console serves. Returns when any of its threads stops,
    /// so the caller can exit and let the service manager restart it.
    pub fn wait(self) -> Result<()> {
        Err(Error::Stopped(self.supervisor.wait()))
    }
}

/// The latest board and whether it is current, as the stream sends them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Published {
    /// Counts board builds, to tell stream events apart.
    pub build: u64,
    pub board: Option<Arc<Board>>,
    /// The change feed is open and the last build read the store.
    pub connected: bool,
}

/// Serve on an already bound listener. The board, its change feed and the
/// HTTP server run on background threads for the life of the process.
pub fn start(listener: std::net::TcpListener, options: Options) -> Result<Running> {
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let rebuilds = Arc::new(Rebuilds::default());
    let connected = Arc::new(AtomicBool::new(false));
    let (publish, published) = watch::channel(Published::default());
    let supervisor = Supervisor::new();
    let Options {
        store,
        assets,
        models,
    } = options;
    {
        let (store, rebuilds, connected) = (store.clone(), rebuilds.clone(), connected.clone());
        supervisor.spawn("feed", move || feed::run(store, rebuilds, connected))?;
    }
    supervisor.spawn("board", move || {
        let mut board = Builder {
            store,
            models,
            reader: None,
            rows: None,
        };
        let mut retry = feed::Backoff::new();
        while let Some(change) = rebuilds.next() {
            let built = board.build(change);
            let live = built.is_ok() && connected.load(Ordering::SeqCst);
            match &built {
                Ok(_) => retry.reset(),
                Err(error) => eprintln!("console board unavailable: {}", describe(error)),
            }
            publish.send_if_modified(|current| {
                let mut changed = current.connected != live;
                current.connected = live;
                if let Ok(board) = &built
                    && current
                        .board
                        .as_deref()
                        .is_none_or(|shown| !same_content(shown, board))
                {
                    current.build += 1;
                    current.board = Some(Arc::new(board.clone()));
                    changed = true;
                }
                changed
            });
            // The feed rebuilds after it reconnects. While it is connected, a
            // failed build (a schema tl3d hasn't migrated yet) retries itself.
            if built.is_err() && connected.load(Ordering::SeqCst) {
                std::thread::sleep(retry.delay());
                rebuilds.request(Change::World);
            }
        }
    })?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tl3-console-http")
        .enable_all()
        .build()?;
    let app = server::router(published, &assets);
    supervisor.spawn("http", move || {
        let served = runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            axum::serve(listener, app).await
        });
        if let Err(error) = served {
            eprintln!("console HTTP server stopped: {error}");
        }
    })?;
    Ok(Running {
        address,
        supervisor,
    })
}

/// Keeps the last read rows so a progress tick rereads progress alone.
struct Builder {
    store: tl3_config::Store,
    models: Option<tl3_config::Models>,
    reader: Option<Reader>,
    rows: Option<Rows>,
}

impl Builder {
    /// Read the board in one snapshot. A broken session is replaced once.
    fn build(&mut self, change: Change) -> tl3_controller::Result<Board> {
        let mut retried = false;
        loop {
            match self.read(change) {
                Err(_) if !retried => {
                    retried = true;
                    self.reader = None;
                }
                result => return result,
            }
        }
    }

    fn read(&mut self, change: Change) -> tl3_controller::Result<Board> {
        if self.reader.is_none() {
            self.reader = Some(Reader::open(&self.store)?);
        }
        let session = self.reader.as_mut().expect("an open reader");
        let mut tx = session.transaction()?;
        let rows = match (change, self.rows.take()) {
            (Change::Progress, Some(mut rows)) => {
                rows.refresh_progress(&mut tx)?;
                rows
            }
            _ => Rows::read(&mut tx)?,
        };
        tx.commit().map_err(tl3_store::Error::from)?;
        let board = rows.project(now_ms(), self.models.as_ref());
        self.rows = Some(rows);
        Ok(board)
    }
}

/// An error and its causes on one line, for the service log.
pub(crate) fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        // Wrappers that display their cause would repeat it.
        let cause_text = cause.to_string();
        if !text.ends_with(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Boards that differ only in when they were built show the same thing.
fn same_content(shown: &Board, built: &Board) -> bool {
    shown.revision == built.revision
        && shown.cards == built.cards
        && shown.flow == built.flow
        && shown.unassigned_incidents == built.unassigned_incidents
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}
