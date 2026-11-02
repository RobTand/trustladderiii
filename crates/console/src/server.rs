//! HTTP: the board, its event stream and the built web app.

use crate::{
    Published,
    api::{DatabaseStatus, PatchEvent},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::get,
};
use futures_util::stream::{self, Stream};
use std::{collections::VecDeque, convert::Infallible, path::Path, sync::Arc, time::Duration};
use tl3_controller::board::Board;
use tokio::sync::watch;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

const CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
/// EventSource waits this long before reconnecting after a dropped stream.
const RETRY: Duration = Duration::from_secs(1);

pub(crate) fn router(published: watch::Receiver<Published>, assets: &Path) -> Router {
    let app = ServeDir::new(assets).fallback(ServeFile::new(assets.join("index.html")));
    Router::new()
        .route("/api/board", get(board))
        .route("/api/stream", get(events))
        .route("/api/{*rest}", get(not_found))
        .fallback_service(app)
        .with_state(published)
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
}

async fn board(State(published): State<watch::Receiver<Published>>) -> Response {
    let board = published.borrow().board.clone();
    match board {
        Some(board) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(board.as_ref().clone()),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "The board has not been read from the store yet.\n",
        )
            .into_response(),
    }
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// Every connection, including a reconnect that sends `Last-Event-ID`,
/// starts with a full snapshot, then gets a patch for each newer board.
async fn events(
    State(published): State<watch::Receiver<Published>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let client = Client {
        published,
        shown: None,
        shown_id: String::new(),
        connected: None,
        queue: VecDeque::new(),
        retry_sent: false,
    };
    Sse::new(stream::unfold(client, |mut client| async move {
        let event = client.next().await?;
        Some((Ok(event), client))
    }))
}

struct Client {
    published: watch::Receiver<Published>,
    shown: Option<Arc<Board>>,
    /// The event id of `shown`, which the next patch names as its base.
    shown_id: String,
    connected: Option<bool>,
    queue: VecDeque<Event>,
    retry_sent: bool,
}

impl Client {
    async fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Some(self.with_retry(event));
            }
            let current = self.published.borrow_and_update().clone();
            if self.connected != Some(current.connected) {
                self.connected = Some(current.connected);
                self.queue.push_back(json(
                    Event::default().event("database"),
                    &DatabaseStatus {
                        connected: current.connected,
                    },
                ));
            }
            if let Some(board) = current.board
                && !self
                    .shown
                    .as_ref()
                    .is_some_and(|shown| Arc::ptr_eq(shown, &board))
            {
                let id = format!("{}.{}", board.revision, current.build);
                self.queue.push_back(match &self.shown {
                    None => json(Event::default().event("snapshot").id(&id), board.as_ref()),
                    Some(shown) => json(
                        Event::default().event("patch").id(&id),
                        &PatchEvent {
                            base: std::mem::take(&mut self.shown_id),
                            patch: board.patch_since(shown),
                        },
                    ),
                });
                self.shown = Some(board);
                self.shown_id = id;
            }
            if self.queue.is_empty() && self.published.changed().await.is_err() {
                return None;
            }
        }
    }

    fn with_retry(&mut self, event: Event) -> Event {
        if std::mem::replace(&mut self.retry_sent, true) {
            event
        } else {
            event.retry(RETRY)
        }
    }
}

fn json(event: Event, data: &impl serde::Serialize) -> Event {
    event
        .json_data(data)
        .expect("board types serialize to JSON")
}
