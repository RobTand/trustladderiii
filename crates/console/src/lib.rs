//! The TL3 operator console (PRD #327): a read-only board, served apart from
//! tl3d. It reads Postgres through read-only sessions and never writes the
//! world snapshot or the event log.

#![forbid(unsafe_code)]

pub mod api;
mod feed;
mod rebuild;
mod server;

use std::{net::SocketAddr, path::PathBuf};

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
}

impl Running {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Block while the console serves.
    pub fn wait(self) {
        todo!()
    }
}

/// Serve on an already bound listener. The board, its change feed and the
/// HTTP server run on background threads for the life of the process.
pub fn start(listener: std::net::TcpListener, options: Options) -> Result<Running> {
    let _ = (listener, options);
    todo!()
}
