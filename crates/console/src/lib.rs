//! The TL3 operator console (PRD #327): a read-only board, served apart from
//! tl3d. It reads Postgres through read-only sessions and never writes the
//! world snapshot or the event log.

#![forbid(unsafe_code)]

pub mod api;
mod feed;
mod rebuild;
mod server;
mod supervise;

use rebuild::Rebuilds;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tl3_controller::board::{self, Board};
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
    server: thread::JoinHandle<std::io::Result<()>>,
}

impl Running {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Block while the console serves.
    pub fn wait(self) -> Result<()> {
        self.server
            .join()
            .map_err(|_| Error::Invalid("the console's HTTP server panicked"))??;
        Ok(())
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
    feed::spawn(options.store.clone(), rebuilds.clone(), connected.clone())?;
    let Options {
        store,
        assets,
        models,
    } = options;
    thread::Builder::new()
        .name("tl3-console-board".into())
        .spawn(move || {
            let mut reader = None;
            while rebuilds.next() {
                let built = build(&mut reader, &store, models.as_ref());
                if let Err(error) = &built {
                    eprintln!("console board unavailable: {error}");
                }
                let live = built.is_ok() && connected.load(Ordering::SeqCst);
                publish.send_if_modified(|current| {
                    let mut changed = current.connected != live;
                    current.connected = live;
                    if let Ok(board) = built
                        && current
                            .board
                            .as_deref()
                            .is_none_or(|shown| !same_content(shown, &board))
                    {
                        current.build += 1;
                        current.board = Some(Arc::new(board));
                        changed = true;
                    }
                    changed
                });
            }
        })?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tl3-console-http")
        .enable_all()
        .build()?;
    let app = server::router(published, &assets);
    let server = thread::Builder::new()
        .name("tl3-console-http".into())
        .spawn(move || {
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                axum::serve(listener, app).await
            })
        })?;
    Ok(Running { address, server })
}

/// Read the board in one snapshot. A broken session is replaced once.
fn build(
    reader: &mut Option<Reader>,
    store: &tl3_config::Store,
    models: Option<&tl3_config::Models>,
) -> tl3_controller::Result<Board> {
    let mut retried = false;
    loop {
        if reader.is_none() {
            *reader = Some(Reader::open(store)?);
        }
        let session = reader.as_mut().expect("an open reader");
        let result = session
            .transaction()
            .map_err(Into::into)
            .and_then(|mut tx| {
                let board = board::read(&mut tx, now_ms(), models)?;
                tx.commit().map_err(tl3_store::Error::from)?;
                Ok(board)
            });
        match result {
            Err(_) if !retried => {
                retried = true;
                *reader = None;
            }
            result => return result,
        }
    }
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
