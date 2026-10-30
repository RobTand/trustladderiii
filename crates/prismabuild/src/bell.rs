use crate::{ActionKey, ActionRecord, Error, PrismaBuild};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize)]
struct Cursor {
    epoch: String,
    seq: u64,
}

impl Cursor {
    fn validate(&self) -> Result<(), Error> {
        if self.epoch.len() != 32
            || !self
                .epoch
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Bell("invalid epoch".into()));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Reply {
    #[serde(flatten)]
    cursor: Cursor,
    #[serde(default)]
    reset: bool,
    events: Option<Vec<Hint>>,
}

#[derive(Deserialize)]
struct Hint {
    seq: u64,
    dir: String,
    key: ActionKey,
}

#[derive(Debug)]
pub struct Changes {
    /// Current observations for every awaited key, including keys with no record.
    pub records: BTreeMap<ActionKey, Option<ActionRecord>>,
    pub reset: bool,
    pub bell_available: bool,
}

pub struct Watcher {
    client: PrismaBuild,
    cursor: Option<Cursor>,
    retry: Duration,
    last_reply: Option<tokio::time::Instant>,
}

impl PrismaBuild {
    pub fn watcher(&self) -> Watcher {
        Watcher {
            client: self.clone(),
            cursor: None,
            retry: Duration::from_millis(250),
            last_reply: None,
        }
    }
}

impl Watcher {
    async fn request(&self, wait: Duration) -> Result<Reply, Error> {
        let mut url = self
            .client
            .bell_url
            .join(if self.cursor.is_some() {
                "v1/wait"
            } else {
                "v1/now"
            })
            .map_err(|e| Error::Bell(e.to_string()))?;
        if let Some(cursor) = &self.cursor {
            let server_wait =
                (wait.as_secs_f64() - 0.05_f64.min(wait.as_secs_f64() / 10.0)).max(0.0);
            url.query_pairs_mut()
                .append_pair("since", &format!("{}:{}", cursor.epoch, cursor.seq))
                .append_pair(
                    "dirs",
                    "ready,claimed,done,failed,withdrawn,attempts,publication-endings",
                )
                // No key filter avoids PB's 128-key filter limit. Records stay authoritative.
                .append_pair("keys", "")
                .append_pair("timeout", &server_wait.to_string());
        }
        let timeout = if self.cursor.is_some() {
            wait
        } else {
            wait.min(Duration::from_secs(1))
        };
        let response = self
            .client
            .http
            .get(url)
            .header(
                "X-PrismaBuild-Queue",
                self.client.options.queue_root.to_str().unwrap(),
            )
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| Error::Bell(e.to_string()))?
            .error_for_status()
            .map_err(|e| Error::Bell(e.to_string()))?;
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Bell(e.to_string()))?
        {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(Error::Bell("reply exceeds 1 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let reply: Reply =
            serde_json::from_slice(&bytes).map_err(|e| Error::Bell(e.to_string()))?;
        reply.cursor.validate()?;
        if let Some(cursor) = &self.cursor
            && !reply.reset
        {
            let Some(events) = &reply.events else {
                return Err(Error::Bell("missing events".into()));
            };
            if reply.cursor.epoch != cursor.epoch
                || reply.cursor.seq < cursor.seq
                || events.iter().any(|hint| {
                    hint.seq <= cursor.seq
                        || hint.seq > reply.cursor.seq
                        || hint.key.as_str().is_empty()
                        || ![
                            "ready",
                            "claimed",
                            "done",
                            "failed",
                            "withdrawn",
                            "deferred",
                            "intent",
                            "attempts",
                            "publication-endings",
                        ]
                        .contains(&hint.dir.as_str())
                })
            {
                return Err(Error::Bell("invalid cursor or event".into()));
            }
        }
        Ok(reply)
    }

    /// Capture the cursor before reads, then reread every key after each bell reply.
    /// On transport failure, reread with capped backoff. Silence never declares loss.
    pub async fn next(
        &mut self,
        awaited: &BTreeSet<ActionKey>,
        patience: Duration,
    ) -> Result<Changes, Error> {
        if patience.is_zero() {
            return Err(Error::InvalidInput("bell patience must be positive".into()));
        }
        let patience = patience.min(Duration::from_secs(300));
        if let Some(last) = self.last_reply {
            let delay = self.retry.saturating_sub(last.elapsed()).min(patience);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        let started = tokio::time::Instant::now();
        let reply = self.request(patience).await;
        let initial = self.cursor.is_none();
        let available = reply.is_ok();
        let reset = initial || reply.as_ref().is_ok_and(|r| r.reset) || !available;
        let records = self.client.reconcile(awaited).await?;
        self.cursor = reply.ok().map(|r| r.cursor);
        self.retry = if started.elapsed() < Duration::from_millis(250) {
            (self.retry * 2).min(Duration::from_secs(5))
        } else {
            Duration::from_millis(250)
        };
        self.last_reply = Some(tokio::time::Instant::now());
        Ok(Changes {
            records,
            reset,
            bell_available: available,
        })
    }
}
