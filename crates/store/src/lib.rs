#![forbid(unsafe_code)]

mod prompts;
mod types;
pub use prompts::*;
pub use types::*;

use postgres::{Client, IsolationLevel, NoTls, Row};
pub use postgres::{Error as DatabaseError, Transaction};
use std::{fs, time::Duration};

/// #766: production query exposed for executable EXPLAIN regression evidence.
pub const DISPATCH_EVIDENCE_QUERY: &str = "SELECT
    EXISTS (SELECT 1 FROM tl3_dispatch_evidence WHERE operation = $1 AND effect_key <> $2)
    OR EXISTS (SELECT 1 FROM tl3_dispatch_evidence WHERE effect_key = $1->>'key' AND effect_key <> $2)";

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Postgres operation failed")]
    Database(#[from] postgres::Error),
    #[error("{0}")]
    Config(#[from] tl3_config::Error),
    #[error("cannot read store.password_file")]
    Password(#[source] std::io::Error),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("world changed; reload revision {revision:?}, epoch {epoch:?}")]
    Stale { revision: Revision, epoch: Epoch },
    #[error("illegal effect transition {from:?} -> {to:?}")]
    IllegalTransition { from: EffectState, to: EffectState },
    #[error("effect missing or its delivery state changed")]
    EffectConflict,
    #[error("event key {key} already has a different payload (stored md5 {stored}, new md5 {new})")]
    EventKeyReused {
        key: String,
        stored: String,
        new: String,
    },
    #[error("credit key already has a different root or amount")]
    CreditConflict,
    #[error("active effect deliveries must drain before an upgrade")]
    ActiveDeliveries,
    #[error("database migration differs from this binary or is newer")]
    MigrationMismatch,
    #[error("database not initialized")]
    NotInitialized,
    #[error("schema version {actual}, expected {expected}")]
    SchemaVersion { actual: i32, expected: i32 },
    #[error("store counter exhausted")]
    CounterOverflow,
}

/// One synchronous connection. The controller owns its writer; readers can open others.
pub struct Store {
    client: Client,
    settings: tl3_config::Store,
    boundary_hook: Option<BoundaryHook>,
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Store>();
};

/// Write boundaries used by process crash simulations. Migration setup and
/// read-only transactions are outside the controller save-point contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavePoint {
    EventWritten,
    RowsWritten,
    EffectsWritten,
    TransitionsWritten,
    CreditWritten,
    BeforeCommit,
    AfterCommit,
    InternBeforeCommit,
    InternAfterCommit,
    ReceiveIngressBeforeCommit,
    ReceiveIngressAfterCommit,
    ConsumeIngressBeforeCommit,
    ConsumeIngressAfterCommit,
    ReceiveCatchUpBeforeCommit,
    ReceiveCatchUpAfterCommit,
    IngressApplied,
}

type BoundaryHook = Box<dyn FnMut(SavePoint, &str, &serde_json::Value) + Send>;

fn boundary(
    hook: &mut Option<BoundaryHook>,
    point: SavePoint,
    key: &str,
    event: &serde_json::Value,
) {
    if let Some(hook) = hook {
        hook(point, key, event);
    }
}

const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/0001_store.sql")),
    (2, include_str!("../migrations/0002_world_arrival.sql")),
    (3, include_str!("../migrations/0003_workers.sql")),
    (4, include_str!("../migrations/0004_ingress.sql")),
    (5, include_str!("../migrations/0005_worker_retirement.sql")),
    (6, include_str!("../migrations/0006_worker_wait.sql")),
    (7, include_str!("../migrations/0007_hindsight.sql")),
    (8, include_str!("../migrations/0008_memory_parking.sql")),
    (9, include_str!("../migrations/0009_memory_recovery.sql")),
    (10, include_str!("../migrations/0010_change_feed.sql")),
    (11, include_str!("../migrations/0011_console_commands.sql")),
    (12, include_str!("../migrations/0012_prompts.sql")),
    (13, include_str!("../migrations/0013_postmortems.sql")),
    (14, include_str!("../migrations/0014_memory_curations.sql")),
    (
        15,
        include_str!("../migrations/0015_postmortem_results.sql"),
    ),
    (17, include_str!("../migrations/0017_worker_facts.sql")),
    (18, include_str!("../migrations/0018_dispatch_evidence.sql")),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ingress {
    pub sequence: i64,
    pub source: String,
    pub delivery_key: String,
    pub event_name: String,
    pub payload: Vec<u8>,
}

fn ingress_row(row: Row) -> Ingress {
    Ingress {
        sequence: row.get(0),
        source: row.get(1),
        delivery_key: row.get(2),
        event_name: row.get(3),
        payload: row.get(4),
    }
}

fn retire_ingress(tx: &mut Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM tl3_ingress WHERE consumed AND sequence <= (SELECT MIN(sequence) FROM tl3_ingress_consumers)", &[])?;
    Ok(())
}

impl Store {
    /// Open an independent read-only session without transferring this writer's ownership.
    pub fn reader(&self) -> Result<Reader> {
        Reader::open(&self.settings)
    }

    /// Independent of kernel revisions. The immutable operation ID survives lost acknowledgements.
    pub fn queue_memory(&mut self, operation_id: &str, item: &serde_json::Value) -> Result<()> {
        let mut tx = self.client.transaction()?;
        queue_memory_in(&mut tx, operation_id, item)?;
        tx.commit()?;
        Ok(())
    }

    /// #422: record one decision about a memory. A replayed operation is ignored.
    pub fn record_memory_curation(
        &mut self,
        operation_id: &str,
        curation: &MemoryCuration,
    ) -> Result<()> {
        self.client.execute(
            "INSERT INTO tl3_memory_curations (operation_id, memory_id, state, verdict, source, request, run, claim, reason, evidence) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (operation_id) DO NOTHING",
            &[&operation_id, &curation.memory_id, &curation.state, &curation.verdict, &curation.source,
              &curation.request, &curation.run, &curation.claim, &curation.reason, &curation.evidence],
        )?;
        Ok(())
    }

    /// Every recorded memory curation, oldest first, for the audit trail.
    pub fn memory_curations(&mut self) -> Result<Vec<serde_json::Value>> {
        Ok(self.client.query(
            "SELECT json_build_object('sequence', sequence, 'memory_id', memory_id, 'state', state, \
             'verdict', verdict, 'source', source, 'request', request, 'run', run, 'claim', claim, \
             'reason', reason, 'evidence', evidence, 'recorded_at', recorded_at, 'delivered_at', delivered_at, \
             'http_status', http_status, 'announced_at', announced_at)::jsonb FROM tl3_memory_curations ORDER BY sequence",
            &[],
        )?
        .into_iter()
        .map(|row| row.get(0))
        .collect())
    }

    /// Accepted curations Hindsight hasn't acknowledged, in decision order:
    /// (sequence, memory id, state, reason).
    pub fn pending_memory_curations(&mut self) -> Result<Vec<(i64, String, String, String)>> {
        Ok(self.client.query(
            "SELECT sequence, memory_id, state, reason FROM tl3_memory_curations \
             WHERE verdict = 'accepted' AND state <> 'hidden' AND delivered_at IS NULL AND http_status IS NULL \
             ORDER BY sequence LIMIT 64",
            &[],
        )?
        .into_iter()
        .map(|row| (row.get(0), row.get(1), row.get(2), row.get(3)))
        .collect())
    }

    /// Hindsight acknowledged the curation, or refused it for good with `status`.
    pub fn settle_memory_curation(&mut self, sequence: i64, status: Option<u16>) -> Result<()> {
        let mut tx = self.client.transaction()?;
        match status {
            None => {
                tx.execute(
                    "UPDATE tl3_memory_curations SET delivered_at = CURRENT_TIMESTAMP WHERE sequence = $1",
                    &[&sequence],
                )?;
                tx.execute("DELETE FROM tl3_memory_health WHERE singleton", &[])?;
            }
            Some(status) => {
                tx.execute(
                    "UPDATE tl3_memory_curations SET http_status = $2 WHERE sequence = $1",
                    &[&sequence, &(status as i32)],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Of `ids`, those TL3 keeps out of recall: the latest accepted decision hides
    /// the memory, or retires it before Hindsight has acknowledged. Delivered
    /// retirements Hindsight filters itself.
    pub fn withheld_memories(
        &mut self,
        ids: &[String],
    ) -> Result<std::collections::BTreeSet<String>> {
        Ok(self.client.query(
            "SELECT memory_id FROM (SELECT DISTINCT ON (memory_id) memory_id, state, delivered_at \
             FROM tl3_memory_curations WHERE verdict = 'accepted' AND memory_id = ANY($1) \
             ORDER BY memory_id, sequence DESC) latest \
             WHERE state = 'hidden' OR (state = 'invalidated' AND delivered_at IS NULL)",
            &[&ids],
        )?
        .into_iter()
        .map(|row| row.get(0))
        .collect())
    }

    /// Accepted decisions about one memory since its last revert, oldest first:
    /// (state, source, request).
    pub fn memory_signals(
        &mut self,
        memory_id: &str,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        Ok(self.client.query(
            "SELECT state, source, request FROM tl3_memory_curations WHERE verdict = 'accepted' \
             AND memory_id = $1 AND sequence > COALESCE((SELECT max(sequence) FROM tl3_memory_curations \
             WHERE verdict = 'accepted' AND memory_id = $1 AND state = 'valid'), 0) ORDER BY sequence",
            &[&memory_id],
        )?
        .into_iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect())
    }

    /// Hides and retirements not yet posted on the operations issue:
    /// (sequence, operation id, row as in `memory_curations`).
    pub fn unannounced_memory_curations(
        &mut self,
    ) -> Result<Vec<(i64, String, serde_json::Value)>> {
        Ok(self.client.query(
            "SELECT sequence, operation_id, json_build_object('memory_id', memory_id, 'state', state, \
             'source', source, 'request', request, 'run', run, 'claim', claim, 'evidence', evidence)::jsonb \
             FROM tl3_memory_curations WHERE verdict = 'accepted' AND state <> 'valid' AND announced_at IS NULL \
             ORDER BY sequence LIMIT 64",
            &[],
        )?
        .into_iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect())
    }

    pub fn announce_memory_curation(&mut self, sequence: i64) -> Result<()> {
        self.client.execute(
            "UPDATE tl3_memory_curations SET announced_at = CURRENT_TIMESTAMP WHERE sequence = $1",
            &[&sequence],
        )?;
        Ok(())
    }

    /// A cursor bounds each read without retaining the history of sent records.
    pub fn pending_memories(
        &mut self,
        after: i64,
    ) -> Result<Vec<(i64, String, serde_json::Value)>> {
        Ok(self.client.query("SELECT sequence, operation_id, item FROM tl3_memory_outbox WHERE sequence > $1 AND parked_reason IS NULL ORDER BY sequence LIMIT 64", &[&after])?
            .into_iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect())
    }

    pub fn memory_item_bytes(&mut self, item: &serde_json::Value) -> Result<i32> {
        Ok(self
            .client
            .query_one("SELECT octet_length($1::jsonb::text)", &[item])?
            .get(0))
    }

    pub fn park_memory(&mut self, operation_id: &str, status: u16) -> Result<()> {
        self.client.execute("UPDATE tl3_memory_outbox SET parked_reason = 'http', http_status = $2, parked_at = CURRENT_TIMESTAMP WHERE operation_id = $1", &[&operation_id, &(status as i32)])?;
        Ok(())
    }

    pub fn reject_memory_key(&mut self, status: u16) -> Result<()> {
        let mut tx = self.client.transaction()?;
        let raised = tx.execute("INSERT INTO tl3_memory_health (singleton, rejected_status) VALUES (true, $1) ON CONFLICT (singleton) DO NOTHING", &[&(status as i32)])?;
        if raised == 0 {
            tx.execute(
                "UPDATE tl3_memory_health SET rejected_status = $1 WHERE singleton",
                &[&(status as i32)],
            )?;
        }
        tx.commit()?;
        if raised != 0 {
            eprintln!("Operations incident: Hindsight rejected TL3's key");
        }
        Ok(())
    }

    pub fn forget_memory(&mut self, operation_id: &str) -> Result<()> {
        let mut tx = self.client.transaction()?;
        tx.execute(
            "DELETE FROM tl3_memory_outbox WHERE operation_id = $1",
            &[&operation_id],
        )?;
        tx.execute("DELETE FROM tl3_memory_health WHERE singleton", &[])?;
        tx.commit()?;
        Ok(())
    }

    pub fn expire_parked_memories(&mut self) -> Result<()> {
        self.client.execute("DELETE FROM tl3_memory_outbox WHERE parked_reason = 'http' AND parked_at <= CURRENT_TIMESTAMP - INTERVAL '30 days'", &[])?;
        Ok(())
    }

    /// Only a successful flush releases capacity. Share the enqueue lock so the cap holds.
    pub fn replay_capacity_memories(&mut self) -> Result<u64> {
        let mut tx = self.client.transaction()?;
        tx.batch_execute("LOCK TABLE tl3_memory_outbox IN SHARE ROW EXCLUSIVE MODE")?;
        let replayed = tx.execute("UPDATE tl3_memory_outbox SET parked_reason = NULL, parked_at = NULL WHERE sequence IN (SELECT sequence FROM tl3_memory_outbox WHERE parked_reason = 'capacity' ORDER BY sequence LIMIT (SELECT GREATEST(0, 10000 - count(*)) FROM tl3_memory_outbox WHERE parked_reason IS NULL))", &[])?;
        tx.commit()?;
        Ok(replayed)
    }

    /// Replace a failed ingress session. The controller's ownership session is never reconnected.
    pub fn reconnect(&mut self) -> Result<()> {
        let mut store = Self::open(&self.settings)?;
        store.boundary_hook = self.boundary_hook.take();
        *self = store;
        Ok(())
    }

    /// Sequence cursors require one connection to insert all GitHub deliveries, including
    /// catch-up. Concurrent inserters could commit a lower sequence after a cursor passed it.
    pub fn ingress_after(&mut self, sequence: i64) -> Result<Vec<Ingress>> {
        Ok(self.client.query("SELECT sequence, source, delivery_key, event_name, payload FROM tl3_ingress WHERE sequence > $1 ORDER BY sequence LIMIT 100", &[&sequence])?.into_iter().map(ingress_row).collect())
    }

    /// Register the current translator set before consuming a batch. New names start at
    /// intake's durable position; catch-up supplies recent state, not retained history.
    pub fn register_ingress_translators(&mut self, names: &[&str]) -> Result<()> {
        let mut tx = self.client.transaction()?;
        tx.execute(
            "DELETE FROM tl3_ingress_consumers WHERE name <> 'intake' AND NOT (name = ANY($1))",
            &[&names],
        )?;
        for name in names {
            tx.execute("INSERT INTO tl3_ingress_consumers (name, sequence) SELECT $1, sequence FROM tl3_ingress_consumers WHERE name = 'intake' ON CONFLICT (name) DO NOTHING", &[name])?;
        }
        retire_ingress(&mut tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn translator_cursor(&mut self, name: &str) -> Result<i64> {
        Ok(self
            .client
            .query_one(
                "SELECT sequence FROM tl3_ingress_consumers WHERE name = $1",
                &[&name],
            )?
            .get(0))
    }

    pub fn advance_translator(&mut self, name: &str, sequence: i64) -> Result<()> {
        let mut tx = self.client.transaction()?;
        tx.execute(
            "UPDATE tl3_ingress_consumers SET sequence = GREATEST(sequence, $2) WHERE name = $1",
            &[&name, &sequence],
        )?;
        retire_ingress(&mut tx)?;
        tx.commit()?;
        Ok(())
    }

    /// The unique delivery id, event name and verified raw payload become durable together.
    pub fn receive_ingress(
        &mut self,
        source: &str,
        key: &str,
        event_name: &str,
        payload: &[u8],
    ) -> Result<bool> {
        let mut tx = self.client.transaction()?;
        let inserted = tx.execute(
            "INSERT INTO tl3_ingress (source, delivery_key, event_name, payload) VALUES ($1, $2, $3, $4) ON CONFLICT (source, delivery_key) DO NOTHING",
            &[&source, &key, &event_name, &payload],
        )? == 1;
        let key = format!("ingress:{source}:{key}");
        boundary(
            &mut self.boundary_hook,
            SavePoint::ReceiveIngressBeforeCommit,
            &key,
            &serde_json::Value::Null,
        );
        tx.commit()?;
        boundary(
            &mut self.boundary_hook,
            SavePoint::ReceiveIngressAfterCommit,
            &key,
            &serde_json::Value::Null,
        );
        Ok(inserted)
    }

    pub fn pending_ingress(&mut self) -> Result<Vec<Ingress>> {
        Ok(self.client.query(
            "SELECT sequence, source, delivery_key, event_name, payload FROM tl3_ingress WHERE NOT consumed ORDER BY sequence LIMIT 100",
            &[],
        )?.into_iter().map(ingress_row).collect())
    }

    /// Call only after every input in the delivery has reached the controller store.
    pub fn consume_ingress(&mut self, sequence: i64) -> Result<()> {
        let mut tx = self.client.transaction()?;
        let row = if self.boundary_hook.is_some() {
            tx.query_opt(
                "SELECT source, delivery_key FROM tl3_ingress WHERE sequence = $1",
                &[&sequence],
            )?
        } else {
            None
        };
        let key = row.map(|row| {
            format!(
                "consume-ingress:{}:{}",
                row.get::<_, String>(0),
                row.get::<_, String>(1)
            )
        });
        tx.execute(
            "UPDATE tl3_ingress SET consumed = true WHERE sequence = $1",
            &[&sequence],
        )?;
        tx.execute("UPDATE tl3_ingress_consumers SET sequence = GREATEST(sequence, $1) WHERE name = 'intake'", &[&sequence])?;
        retire_ingress(&mut tx)?;
        if let Some(key) = &key {
            boundary(
                &mut self.boundary_hook,
                SavePoint::ConsumeIngressBeforeCommit,
                key,
                &serde_json::Value::Null,
            );
        }
        tx.commit()?;
        if let Some(key) = &key {
            boundary(
                &mut self.boundary_hook,
                SavePoint::ConsumeIngressAfterCommit,
                key,
                &serde_json::Value::Null,
            );
        }
        Ok(())
    }

    /// The controller has applied every translated input, but has not consumed its delivery.
    pub fn ingress_applied(&mut self, ingress: &Ingress) {
        let key = format!(
            "ingress-applied:{}:{}",
            ingress.source, ingress.delivery_key
        );
        boundary(
            &mut self.boundary_hook,
            SavePoint::IngressApplied,
            &key,
            &serde_json::Value::Null,
        );
    }

    pub fn ingress_cursor(&mut self, source: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .client
            .query_opt(
                "SELECT cursor FROM tl3_ingress_cursors WHERE source = $1",
                &[&source],
            )?
            .map(|row| row.get(0)))
    }

    /// Persist a catch-up pass and its cursor atomically. A partial pass advances neither.
    pub fn receive_catch_up(
        &mut self,
        source: &str,
        deliveries: &[(String, Vec<u8>)],
        cursor: &serde_json::Value,
    ) -> Result<()> {
        let mut tx = self.client.transaction()?;
        for (key, payload) in deliveries {
            tx.execute(
                "INSERT INTO tl3_ingress (source, delivery_key, event_name, payload) VALUES ($1, $2, 'catch-up', $3) ON CONFLICT (source, delivery_key) DO NOTHING",
                &[&source, key, payload],
            )?;
        }
        tx.execute("INSERT INTO tl3_ingress_cursors (source, cursor) VALUES ($1, $2) ON CONFLICT (source) DO UPDATE SET cursor = EXCLUDED.cursor", &[&source, cursor])?;
        let key = format!("catch-up:{source}");
        boundary(
            &mut self.boundary_hook,
            SavePoint::ReceiveCatchUpBeforeCommit,
            &key,
            cursor,
        );
        tx.commit()?;
        boundary(
            &mut self.boundary_hook,
            SavePoint::ReceiveCatchUpAfterCommit,
            &key,
            cursor,
        );
        Ok(())
    }

    /// Uses only the supplied settings. No environment or implicit credential file is read.
    /// Transport is local TCP or a Unix socket, without TLS.
    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        let mut store = Self {
            client: connect(settings, false)?,
            boundary_hook: None,
            settings: settings.clone(),
        };
        if store.client.query_one("SHOW fsync", &[])?.get::<_, &str>(0) != "on" {
            return Err(Error::Invalid("store requires Postgres fsync = on"));
        }
        store.client.batch_execute("SET synchronous_commit = on")?;
        store.migrate()?;
        Ok(store)
    }

    /// Install before opening the controller to include bootstrap and intake.
    /// A hook must return normally or terminate the process; unwinding is not a crash.
    pub fn set_boundary_hook(
        &mut self,
        hook: impl FnMut(SavePoint, &str, &serde_json::Value) + Send + 'static,
    ) {
        self.boundary_hook = Some(Box::new(hook));
    }

    /// Hold a session advisory lock for the sole controller writer. Status readers
    /// use ordinary connections. Dropping this connection releases ownership.
    pub fn claim_controller(&mut self) -> Result<()> {
        if !self
            .client
            .query_one(
                "SELECT pg_try_advisory_lock($1)",
                &[&0x544c33434f4e5452_i64],
            )?
            .get::<_, bool>(0)
        {
            return Err(Error::Invalid(
                "controller ownership is still held by a Postgres session; exit and let the service manager restart after that session ends",
            ));
        }
        Ok(())
    }

    fn migrate(&mut self) -> Result<()> {
        let mut tx = self.client.transaction()?;
        tx.query_one(
            "SELECT pg_advisory_xact_lock($1)",
            &[&0x544c3353544f5245_i64],
        )?;
        tx.batch_execute("CREATE TABLE IF NOT EXISTS tl3_schema_migrations (version integer PRIMARY KEY, sql text NOT NULL)")?;
        let applied = tx.query(
            "SELECT version, sql FROM tl3_schema_migrations ORDER BY version",
            &[],
        )?;
        for row in &applied {
            let version: i32 = row.get(0);
            let sql: String = row.get(1);
            if !MIGRATIONS
                .iter()
                .any(|&(v, text)| v == version && text == sql)
            {
                return Err(Error::MigrationMismatch);
            }
        }
        for &(version, sql) in MIGRATIONS {
            if !applied.iter().any(|row| row.get::<_, i32>(0) == version) {
                tx.batch_execute(sql)?;
                tx.execute(
                    "INSERT INTO tl3_schema_migrations VALUES ($1, $2)",
                    &[&version, &sql],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Read a consistent world, pending outbox, ledger and intern dictionary, without replay.
    pub fn load(&mut self) -> Result<Snapshot> {
        let mut tx = read_transaction(&mut self.client)?;
        let snapshot = load_snapshot(&mut tx)?;
        tx.commit()?;
        Ok(snapshot)
    }

    /// Identical events return the original receipt, even if this caller's revision is stale.
    /// On any error the transaction rolls back, including the event and all earlier changes.
    pub fn commit(&mut self, change: &Commit) -> Result<CommitReceipt> {
        let mut tx = self.client.transaction()?;
        let (revision, epoch) = metadata(&mut tx, true)?;
        if let Some(row) = tx.query_opt(
            // Only a differing replay reads the stored payload and digests.
            "SELECT revision, epoch, result, payload = $2,
                CASE WHEN payload = $2 THEN NULL ELSE md5(payload::text) END,
                CASE WHEN payload = $2 THEN NULL ELSE md5($2::jsonb::text) END,
                CASE WHEN payload = $2 THEN NULL ELSE payload END
             FROM tl3_events WHERE event_key = $1",
            &[&change.event_key, &change.event],
        )? {
            if !row.get::<_, bool>(3) {
                let (key, stored, new) = (
                    &change.event_key,
                    row.get::<_, String>(4),
                    row.get::<_, String>(5),
                );
                if !same_fact(&row.get(6), &change.event) {
                    eprintln!(
                        "event key {key} reused for a different fact: stored payload md5 {stored}, new payload md5 {new}"
                    );
                    return Err(Error::EventKeyReused {
                        key: key.clone(),
                        stored,
                        new,
                    });
                }
                eprintln!(
                    "event key {key} replayed in a newer shape; keeping the stored receipt: stored payload md5 {stored}, new payload md5 {new}"
                );
            }
            return receipt(row);
        }
        if revision != change.expected_revision || epoch != change.epoch {
            return Err(Error::Stale { revision, epoch });
        }
        let revision = revision.next()?;
        let next_epoch = match change.next_epoch {
            Some(next) if next == epoch.next()? => next,
            Some(_) => return Err(Error::Invalid("next epoch must advance by exactly one")),
            None => epoch,
        };
        if next_epoch != epoch && tx.query_one("SELECT EXISTS (SELECT 1 FROM tl3_effects WHERE state IN ('Sent', 'Uncertain', 'Cancelling'))", &[])?.get::<_, bool>(0) {
            return Err(Error::ActiveDeliveries);
        }
        let committed = receipt(tx.query_one("INSERT INTO tl3_events (event_key, revision, epoch, payload, result) VALUES ($1, $2, $3, $4, $5) RETURNING revision, epoch, result",
            &[&change.event_key, &revision.get(), &next_epoch.get(), &change.event, &change.result])?)?;
        boundary(
            &mut self.boundary_hook,
            SavePoint::EventWritten,
            &change.event_key,
            &change.event,
        );
        for row in &change.rows {
            match row {
                RowChange::Put { table, id, value } => {
                    tx.execute("INSERT INTO tl3_world (table_name, row_id, value, version) VALUES ($1, $2, $3, $4) ON CONFLICT (table_name, row_id) DO UPDATE SET value = EXCLUDED.value, version = EXCLUDED.version",
                        &[table, id, value, &revision.get()])?;
                    if table == "nodes" && value["phase"].get("Closed").is_some() {
                        tx.execute("DELETE FROM tl3_worker_summary WHERE node_id = $1", &[id])?;
                    }
                }
                RowChange::Delete { table, id } => {
                    tx.execute(
                        "DELETE FROM tl3_world WHERE table_name = $1 AND row_id = $2",
                        &[table, id],
                    )?;
                    if table == "nodes" {
                        tx.execute("DELETE FROM tl3_worker_summary WHERE node_id = $1", &[id])?;
                    }
                }
            }
        }
        boundary(
            &mut self.boundary_hook,
            SavePoint::RowsWritten,
            &change.event_key,
            &change.event,
        );
        for effect in &change.effects {
            tx.execute("INSERT INTO tl3_effects (effect_key, epoch, kind, payload, created_revision, updated_revision) VALUES ($1, $2, $3, $4, $5, $5)",
                &[&effect.key, &next_epoch.get(), &effect.kind, &effect.payload, &revision.get()])?;
        }
        boundary(
            &mut self.boundary_hook,
            SavePoint::EffectsWritten,
            &change.event_key,
            &change.event,
        );
        for transition in &change.transitions {
            if !transition.from.allows(transition.to) {
                return Err(Error::IllegalTransition {
                    from: transition.from,
                    to: transition.to,
                });
            }
            if tx.execute("UPDATE tl3_effects SET state = $1::text::tl3_effect_state, updated_revision = $2 WHERE effect_key = $3 AND state = $4::text::tl3_effect_state",
                &[&transition.to.as_str(), &revision.get(), &transition.key, &transition.from.as_str()])? != 1 {
                return Err(Error::EffectConflict);
            }
        }
        boundary(
            &mut self.boundary_hook,
            SavePoint::TransitionsWritten,
            &change.event_key,
            &change.event,
        );
        for entry in &change.credit {
            let inserted = tx.execute("INSERT INTO tl3_credit (node_id, reason, root_id, amount, revision) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (node_id, reason) DO NOTHING",
                &[&entry.node, &entry.reason, &entry.root, &entry.amount, &revision.get()])?;
            if inserted == 0 {
                let prior = tx.query_one(
                    "SELECT root_id, amount FROM tl3_credit WHERE node_id = $1 AND reason = $2",
                    &[&entry.node, &entry.reason],
                )?;
                if prior.get::<_, &str>(0) != entry.root || prior.get::<_, i64>(1) != entry.amount {
                    return Err(Error::CreditConflict);
                }
            }
        }
        for owed in &change.postmortems {
            tx.execute("INSERT INTO tl3_postmortems (node_id, trigger, reason, sample_share, repo, issue, title, revision, owed_at) SELECT $1, $2, $3, $4, $5, $6, $7, revision, committed_at FROM tl3_events WHERE revision = $8 ON CONFLICT (node_id, trigger) DO NOTHING",
                &[&owed.node, &owed.trigger, &owed.reason, &owed.sample_share, &owed.repo, &owed.issue, &owed.title, &revision.get()])?;
        }
        for grade in &change.grades {
            insert_grade(&mut tx, grade, revision.get())?;
        }
        for update in &change.postmortem_updates {
            apply_postmortem(&mut tx, update, revision.get())?;
        }
        boundary(
            &mut self.boundary_hook,
            SavePoint::CreditWritten,
            &change.event_key,
            &change.event,
        );
        if next_epoch != epoch {
            tx.execute(
                "UPDATE tl3_effects SET epoch = $1, updated_revision = $2 WHERE state = 'Intended'",
                &[&next_epoch.get(), &revision.get()],
            )?;
            // An upgrade cannot introduce a delivery that was picked up under the old epoch.
            if tx.query_one("SELECT EXISTS (SELECT 1 FROM tl3_effects WHERE state IN ('Sent', 'Uncertain', 'Cancelling'))", &[])?.get::<_, bool>(0) {
                return Err(Error::ActiveDeliveries);
            }
        }
        tx.execute(
            "UPDATE tl3_meta SET revision = $1, epoch = $2",
            &[&revision.get(), &next_epoch.get()],
        )?;
        boundary(
            &mut self.boundary_hook,
            SavePoint::BeforeCommit,
            &change.event_key,
            &change.event,
        );
        tx.commit()?;
        boundary(
            &mut self.boundary_hook,
            SavePoint::AfterCommit,
            &change.event_key,
            &change.event,
        );
        Ok(committed)
    }

    pub fn effect(&mut self, key: &str) -> Result<Option<Effect>> {
        self.client.query_opt("SELECT effect_key, epoch, kind, payload, state::text, created_revision, updated_revision FROM tl3_effects WHERE effect_key = $1", &[&key])?
            .map(decode_effect).transpose()
    }

    /// Includes terminal deliveries so callers can audit coverage, not just pending work.
    pub fn effects(&mut self) -> Result<Vec<Effect>> {
        load_effects(&mut self.client)
    }

    /// #766/Q27: migrate at most 256 audit rows per drive, using the revision index.
    /// The cursor commits with the evidence, so a crash neither skips nor double-counts.
    pub fn migrate_dispatch_evidence(&mut self) -> Result<bool> {
        let mut tx = self.client.transaction()?;
        metadata(&mut tx, true)?;
        let row = tx.query_one(
            "SELECT cursor, through_revision FROM tl3_dispatch_migration FOR UPDATE",
            &[],
        )?;
        let (cursor, through): (i64, i64) = (row.get(0), row.get(1));
        if cursor >= through {
            return Ok(false);
        }
        let rows = tx.query("SELECT revision, payload FROM tl3_events WHERE revision > $1 AND revision <= $2 ORDER BY revision LIMIT 256", &[&cursor, &through])?;
        let mut next = through;
        for row in &rows {
            next = row.get(0);
            let event: serde_json::Value = row.get(1);
            if event["to"] == "Sent"
                && let Some(key) = event["effect"].as_str()
            {
                tx.execute("INSERT INTO tl3_dispatch_evidence(effect_key, operation, legacy_sends)
                    SELECT effect_key, tl3_approval_operation(payload), 1 FROM tl3_effects
                    WHERE effect_key = $1 AND payload->'output'->'Effect'->>'kind' = 'PostApproval'
                    ON CONFLICT (effect_key) DO UPDATE SET legacy_sends = tl3_dispatch_evidence.legacy_sends + 1", &[&key])?;
            }
        }
        tx.execute("UPDATE tl3_dispatch_migration SET cursor = $1", &[&next])?;
        tx.commit()?;
        Ok(true)
    }

    /// Conservative dispatch count, including legacy Sent intent before wire evidence existed.
    pub fn approval_dispatch_count(&mut self, key: &str) -> Result<i64> {
        Ok(self
            .client
            .query_opt(
                "SELECT sends + legacy_sends FROM tl3_dispatch_evidence WHERE effect_key = $1",
                &[&key],
            )?
            .map_or(0, |r| r.get(0)))
    }

    /// #766: a replacement delivery inherits dispatch evidence for its operation.
    /// Both positive and negative searches use per-operation/key indexes, never audit scans.
    pub fn dispatched_effect_matching(
        &mut self,
        identity: &serde_json::Value,
        excluding: &str,
    ) -> Result<bool> {
        if self
            .client
            .query_one(
                "SELECT cursor < through_revision FROM tl3_dispatch_migration",
                &[],
            )?
            .get::<_, bool>(0)
        {
            return Err(Error::Invalid("dispatch evidence migration is incomplete"));
        }
        Ok(self
            .client
            .query_one(DISPATCH_EVIDENCE_QUERY, &[identity, &excluding])?
            .get(0))
    }

    /// Retirement commits retain comment IDs without loading their old payloads.
    pub fn comment_retired(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM tl3_events WHERE event_key = $1)",
                &[&format!("retire-comment:{id}")],
            )?
            .get(0))
    }

    /// Match durable event identities and payloads without replaying history.
    pub fn event_matches(&mut self, keys: &[String], value: &serde_json::Value) -> Result<bool> {
        Ok(self.client.query_one(
            "SELECT EXISTS (SELECT 1 FROM tl3_events WHERE event_key = ANY($1) AND payload = $2)",
            &[&keys, &value],
        )?.get(0))
    }

    /// Audit only. A newer kernel must load the snapshot rather than replay these records.
    pub fn events_after(&mut self, revision: Revision, limit: u32) -> Result<Vec<EventRecord>> {
        load_events(&mut self.client, revision, limit)
    }

    /// Materials are supplied by ingress, never fetched or invented by the worker.
    pub fn put_worker_materials(&mut self, node: &str, inputs: &serde_json::Value) -> Result<()> {
        self.client.execute("INSERT INTO tl3_worker_materials VALUES ($1, $2) ON CONFLICT (node_id) DO UPDATE SET inputs = EXCLUDED.inputs", &[&node, inputs])?;
        Ok(())
    }

    pub fn worker_materials(&mut self, node: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .client
            .query_opt(
                "SELECT inputs FROM tl3_worker_materials WHERE node_id = $1",
                &[&node],
            )?
            .map(|r| r.get(0)))
    }

    /// Read the accepted brief, goals, or audit without controller ownership.
    pub fn worker_document(
        &mut self,
        node: &str,
        document: &str,
    ) -> Result<Option<serde_json::Value>> {
        if !matches!(
            document,
            "briefs"
                | "goals"
                | "audits"
                | "maker_replies"
                | "maker_findings"
                | "review_fixes"
                | "maker_session"
                | "reviewer_session"
                | "plans"
                | "findings"
        ) {
            return Err(Error::Invalid("unknown worker document"));
        }
        Ok(self
            .client
            .query_opt(
                "SELECT value FROM tl3_world WHERE table_name = $1 AND row_id = $2",
                &[&document, &node],
            )?
            .map(|r| r.get(0)))
    }

    /// The share of clean merges sampled for a postmortem: a stored setting (#403).
    pub fn postmortem_sample_share(&mut self) -> Result<f64> {
        Ok(self
            .client
            .query_opt(
                "SELECT (value #>> '{}')::double precision FROM tl3_settings WHERE key = 'postmortem_sample_share'",
                &[],
            )?
            .map_or(0.0, |row| row.get(0)))
    }

    /// A node's postmortems, oldest first.
    pub fn postmortems(&mut self, node: &str) -> Result<Vec<PostmortemRecord>> {
        Ok(self
            .client
            .query(
                &format!(
                    "SELECT {POSTMORTEM_RECORD} FROM tl3_postmortems WHERE node_id = $1 ORDER BY id"
                ),
                &[&node],
            )?
            .iter()
            .map(postmortem_record)
            .collect())
    }

    /// The oldest owed postmortem, which the runner claims next (#403). Runs
    /// owed again after a failure wait behind every first run (#486).
    pub fn owed_postmortem(&mut self) -> Result<Option<PostmortemRecord>> {
        Ok(self
            .client
            .query_opt(
                &format!("SELECT {POSTMORTEM_RECORD} FROM tl3_postmortems WHERE status = 'owed' ORDER BY owed_at, id LIMIT 1"),
                &[],
            )?
            .as_ref()
            .map(postmortem_record))
    }

    /// Postmortem claims made since the start of the current UTC day (#486).
    pub fn postmortem_claims_today(&mut self) -> Result<u32> {
        self.postmortem_claims_today_since(0)
    }

    /// Claims today after the last recovery boundary. Earlier claims stay in the ledger.
    pub fn postmortem_claims_today_since(&mut self, since: i64) -> Result<u32> {
        let count: i64 = self
            .client
            .query_one(
                "SELECT count(*) FROM tl3_postmortem_claims WHERE revision > $1 AND claimed_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
                &[&since],
            )?
            .get(0);
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// The failure classes of postmortem runs since the last one that
    /// succeeded, newest first and up to 3, counting only claims committed
    /// after revision `since` (#486).
    pub fn postmortem_failure_streak(&mut self, since: i64) -> Result<Vec<String>> {
        let outcomes = self.client.query(
            "SELECT failure FROM tl3_postmortem_claims WHERE revision > $1 AND outcome IN ('done', 'failed') ORDER BY revision DESC LIMIT 3",
            &[&since],
        )?;
        // A success has no failure class, and ends the streak.
        Ok(outcomes
            .iter()
            .map_while(|row| row.get::<_, Option<String>>(0))
            .collect())
    }

    /// Postmortems claimed by a runner and not yet finished.
    pub fn running_postmortems(&mut self) -> Result<Vec<PostmortemRecord>> {
        Ok(self
            .client
            .query(
                &format!("SELECT {POSTMORTEM_RECORD} FROM tl3_postmortems WHERE status = 'running' ORDER BY id"),
                &[],
            )?
            .iter()
            .map(postmortem_record)
            .collect())
    }

    /// One postmortem with its stored result and recommendations.
    pub fn postmortem(&mut self, id: i64) -> Result<Option<PostmortemDetail>> {
        let mut tx = read_transaction(&mut self.client)?;
        let Some(row) = tx.query_opt(
            &format!("SELECT {POSTMORTEM_RECORD}, repo, issue, title, model, rubric_version, failure, cause, summary, timeline, process_failures, grades, notes, assessment_ids FROM tl3_postmortems WHERE id = $1"),
            &[&id],
        )?
        else {
            tx.commit()?;
            return Ok(None);
        };
        let recommendations = tx
            .query(
                "SELECT fingerprint, rank, phase, kind, target, operation, before, after, evidence, effect FROM tl3_postmortem_recommendations WHERE postmortem_id = $1 ORDER BY rank",
                &[&id],
            )?
            .iter()
            .map(|row| Recommendation {
                fingerprint: row.get(0),
                rank: row.get(1),
                phase: row.get(2),
                kind: row.get(3),
                target: row.get(4),
                operation: row.get(5),
                before: row.get(6),
                after: row.get(7),
                evidence: row.get(8),
                effect: row.get(9),
            })
            .collect();
        tx.commit()?;
        Ok(Some(PostmortemDetail {
            record: postmortem_record(&row),
            repo: row.get("repo"),
            issue: row.get("issue"),
            title: row.get("title"),
            model: row.get("model"),
            rubric_version: row.get("rubric_version"),
            failure: row.get("failure"),
            cause: row.get("cause"),
            summary: row.get("summary"),
            timeline: row.get("timeline"),
            process_failures: row.get("process_failures"),
            grades: row.get("grades"),
            notes: row.get("notes"),
            assessment_ids: row.get("assessment_ids"),
            recommendations,
        }))
    }

    /// A node's code grades, oldest first.
    pub fn code_grades(&mut self, node: &str) -> Result<Vec<CodeGrade>> {
        Ok(self
            .client
            .query(
                "SELECT node_id, trigger, rubric_version, dimension, grade, fact FROM tl3_code_grades WHERE node_id = $1 ORDER BY id",
                &[&node],
            )?
            .iter()
            .map(|row| CodeGrade {
                node: row.get(0),
                trigger: row.get(1),
                rubric_version: row.get(2),
                dimension: row.get(3),
                grade: row.get(4),
                fact: row.get(5),
            })
            .collect())
    }

    /// The phases a node entered, with milliseconds since the Unix epoch, oldest first.
    pub fn phase_history(&mut self, node: &str) -> Result<Vec<(serde_json::Value, i64)>> {
        Ok(self
            .client
            .query(
                "SELECT phase, (extract(epoch FROM since) * 1000)::bigint FROM tl3_phase_history WHERE node_id = $1 ORDER BY since, id",
                &[&node],
            )?
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect())
    }

    /// Context committed with the kernel question and its notice delivery.
    pub fn question_context(&mut self, question: u64) -> Result<Option<serde_json::Value>> {
        Ok(self
            .client
            .query_opt(
                "SELECT value FROM tl3_world WHERE table_name = 'question_context' AND row_id = $1",
                &[&question.to_string()],
            )?
            .map(|row| row.get(0)))
    }

    pub fn worker_facts_recorded(&mut self, key: &str, attempt: i64) -> Result<bool> {
        Ok(self
            .client
            .query_opt(
                "SELECT 1 FROM tl3_worker_runs WHERE run_key = $1 AND attempt = $2",
                &[&key, &attempt],
            )?
            .is_some())
    }

    /// Import a worker-owned terminal record before retiring its attempt files.
    /// A replay is harmless; changing an already imported attempt is refused.
    pub fn record_worker_facts(
        &mut self,
        key: &str,
        node: &str,
        attempt: i64,
        role: &str,
        facts: &WorkerRunFacts,
    ) -> Result<()> {
        facts.validate()?;
        let value = serde_json::to_value(facts).expect("worker facts are JSON values");
        self.client.execute(
            "INSERT INTO tl3_worker_runs (run_key, node_id, attempt, role, facts) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (run_key, attempt) DO NOTHING",
            &[&key, &node, &attempt, &role, &value],
        )?;
        if self.client.query_opt(
            "SELECT 1 FROM tl3_worker_runs WHERE run_key = $1 AND node_id = $2 AND attempt = $3 AND role = $4 AND facts = $5",
            &[&key, &node, &attempt, &role, &value],
        )?.is_none() {
            return Err(Error::Invalid("worker completion record changed"));
        }
        Ok(())
    }

    /// Claim time exists even if the worker never emits progress.
    pub fn claim_worker(&mut self, key: &str, node: &str, epoch: i64, attempt: i64) -> Result<()> {
        self.client.execute("INSERT INTO tl3_worker_progress (run_key, node_id, epoch, attempt, output_offset, unit, started_ms) SELECT $1, $2, $3, $4, 0, '', (extract(epoch FROM clock_timestamp()) * 1000)::bigint FROM tl3_meta WHERE epoch = $3 ON CONFLICT (run_key) DO UPDATE SET attempt = EXCLUDED.attempt, output_offset = 0, unit = '', started_ms = EXCLUDED.started_ms, last_progress_ms = NULL, external_wait = NULL WHERE tl3_worker_progress.epoch = EXCLUDED.epoch AND tl3_worker_progress.attempt < EXCLUDED.attempt", &[&key, &node, &epoch, &attempt])?;
        Ok(())
    }

    /// Replaying an output prefix never refreshes its progress time.
    pub fn record_worker_progress(
        &mut self,
        key: &str,
        attempt: i64,
        offset: i64,
        unit: &str,
        external_wait: Option<&ExternalWait>,
    ) -> Result<()> {
        let wait = external_wait
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| Error::Invalid("invalid external wait"))?;
        self.client.execute("UPDATE tl3_worker_progress SET output_offset = $3, unit = CASE WHEN $5::jsonb IS NULL THEN $4 ELSE unit END, external_wait = $5, last_progress_ms = CASE WHEN $5::jsonb IS NULL THEN (extract(epoch FROM clock_timestamp()) * 1000)::bigint ELSE last_progress_ms END WHERE run_key = $1 AND attempt = $2 AND output_offset < $3 AND epoch = (SELECT epoch FROM tl3_meta)", &[&key, &attempt, &offset, &unit, &wait])?;
        Ok(())
    }

    pub fn worker_progress(&mut self, node: Option<&str>) -> Result<Vec<WorkerProgress>> {
        self.client.query("SELECT run_key, node_id, epoch, attempt, output_offset, unit, started_ms, last_progress_ms, external_wait FROM tl3_worker_progress WHERE ($1::text IS NULL OR node_id = $1) ORDER BY started_ms DESC, run_key", &[&node])?.into_iter().map(|r| Ok(WorkerProgress {
            key: r.get(0), node: r.get(1), epoch: r.get(2), attempt: r.get(3), output_offset: r.get(4), unit: r.get(5), started_ms: r.get(6), last_progress_ms: r.get(7),
            external_wait: r.get::<_, Option<serde_json::Value>>(8).map(serde_json::from_value).transpose().map_err(|_| Error::Invalid("invalid stored external wait"))?,
        })).collect()
    }

    /// Only watched worker deliveries, without loading historical world rows.
    pub fn worker_watches(&mut self) -> Result<Vec<(String, serde_json::Value)>> {
        Ok(self.client.query("SELECT w.row_id, w.value FROM tl3_world w JOIN tl3_effects e ON e.effect_key = w.row_id WHERE w.table_name = 'watches' AND e.kind = 'Workers'", &[])?
            .into_iter().map(|row| (row.get(0), row.get(1))).collect())
    }

    /// Remaining session directories are the cleanup queue, including after a crash.
    pub fn can_retire_worker_session(&mut self, node: &str) -> Result<bool> {
        Ok(self.client.query_one(
            "SELECT EXISTS (SELECT 1 FROM tl3_world WHERE table_name = 'nodes' AND row_id = $1 AND value->'phase' ? 'Closed')
             AND NOT EXISTS (SELECT 1 FROM tl3_world WHERE table_name = 'inbox')
             AND NOT EXISTS (SELECT 1 FROM tl3_worker_progress WHERE node_id = $1)
             AND NOT EXISTS (SELECT 1 FROM tl3_effects WHERE state NOT IN ('Confirmed', 'Failed', 'Cancelled') AND COALESCE(payload #>> '{output,Effect,node}', payload #>> '{output,Notice,node}', payload #>> '{output,Comment,node}') = $1)
             AND NOT EXISTS (SELECT 1 FROM tl3_world WHERE table_name = 'watches' AND COALESCE(value #>> '{output,Effect,node}', value #>> '{output,Notice,node}', value #>> '{output,Comment,node}') = $1)",
            &[&node],
        )?.get(0))
    }

    /// A stale notification cannot revive a terminal, unwatched delivery.
    pub fn worker_reconcilable(&mut self, key: &str) -> Result<bool> {
        Ok(self
            .client
            .query_one(
                "SELECT NOT EXISTS (SELECT 1 FROM tl3_retired_workers WHERE effect_key = $1)",
                &[&key],
            )?
            .get(0))
    }

    /// Terminal effects cannot be dispatched again. Watches retain their evidence.
    pub fn retired_workers(&mut self) -> Result<Vec<String>> {
        Ok(self.client.query("SELECT p.run_key FROM tl3_worker_progress p JOIN tl3_retired_workers e ON e.effect_key = p.run_key WHERE NOT p.retirement_blocked", &[])?
            .into_iter().map(|row| row.get(0)).collect())
    }

    /// Retain evidence whose process identity cannot safely be stopped. Log once at the caller.
    pub fn block_worker_retirement(&mut self, key: &str) -> Result<()> {
        self.client.execute(
            "UPDATE tl3_worker_progress SET retirement_blocked = true WHERE run_key = $1",
            &[&key],
        )?;
        Ok(())
    }

    /// Call only after durable removal of the run directory.
    /// Fold progress into one summary for an open node in the same transaction.
    pub fn forget_worker(&mut self, key: &str) -> Result<()> {
        let mut tx = self.client.transaction()?;
        // Serialize with world commits so closure cannot race summary insertion.
        tx.query_one("SELECT revision FROM tl3_meta FOR UPDATE", &[])?;
        if let Some(row) = tx.query_opt("DELETE FROM tl3_worker_progress p USING tl3_retired_workers e WHERE p.run_key = $1 AND e.effect_key = p.run_key RETURNING p.node_id, p.unit, p.last_progress_ms", &[&key])? {
            let node: String = row.get(0);
            let unit: String = row.get(1);
            if let Some(time) = row.get::<_, Option<i64>>(2) {
                tx.execute("INSERT INTO tl3_worker_summary (node_id, unit, last_progress_ms) SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM tl3_world WHERE table_name = 'nodes' AND row_id = $1 AND NOT (value->'phase' ? 'Closed')) ON CONFLICT (node_id) DO UPDATE SET unit = EXCLUDED.unit, last_progress_ms = EXCLUDED.last_progress_ms WHERE tl3_worker_summary.last_progress_ms < EXCLUDED.last_progress_ms", &[&node, &unit, &time])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn intern_repo(&mut self, owner: &str, name: &str) -> Result<RepoId> {
        Ok(RepoId(self.intern(
            InternKind::Repo,
            &owner.to_ascii_lowercase(),
            &name.to_ascii_lowercase(),
        )?))
    }

    pub fn intern_commit(&mut self, node: &str, sha: &str) -> Result<CommitId> {
        let sha = tl3_worktrees::CommitId::new(sha).map_err(|_| {
            Error::Invalid("commit SHA must be 40 or 64 lowercase hexadecimal digits")
        })?;
        Ok(CommitId(self.intern(
            InternKind::Commit,
            node,
            sha.as_str(),
        )?))
    }

    pub fn intern_path(&mut self, repo: RepoId, path: &str) -> Result<PathId> {
        Ok(PathId(self.intern(
            InternKind::Path,
            &repo.get().to_string(),
            path,
        )?))
    }

    pub fn intern_issue(&mut self, repo: RepoId, number: u64) -> Result<IssueId> {
        if number == 0 {
            return Err(Error::Invalid("issue number must be positive"));
        }
        Ok(IssueId(self.intern(
            InternKind::Issue,
            &repo.get().to_string(),
            &number.to_string(),
        )?))
    }

    pub fn intern_pull_request(&mut self, repo: RepoId, number: u64) -> Result<PullRequestId> {
        if number == 0 {
            return Err(Error::Invalid("PR number must be positive"));
        }
        Ok(PullRequestId(self.intern(
            InternKind::PullRequest,
            &repo.get().to_string(),
            &number.to_string(),
        )?))
    }

    fn intern(&mut self, kind: InternKind, scope: &str, value: &str) -> Result<i64> {
        if scope.is_empty() || value.is_empty() {
            return Err(Error::Invalid("intern scope and value must be nonempty"));
        }
        let select = "SELECT id FROM tl3_intern WHERE kind = $1::text::tl3_intern_kind AND scope = $2 AND value = $3";
        let params: &[&(dyn postgres::types::ToSql + Sync)] = &[&kind.as_str(), &scope, &value];
        if let Some(row) = self.client.query_opt(select, params)? {
            return Ok(row.get(0));
        }
        let key = format!("intern:{}:{scope}:{value}", kind.as_str());
        let mut tx = self.client.transaction()?;
        let inserted = tx.query_opt("INSERT INTO tl3_intern (kind, scope, value) VALUES ($1::text::tl3_intern_kind, $2, $3) ON CONFLICT (kind, scope, value) DO NOTHING RETURNING id", params)?;
        let id = match inserted {
            Some(row) => row.get(0),
            None => tx.query_one(select, params)?.get(0),
        };
        boundary(
            &mut self.boundary_hook,
            SavePoint::InternBeforeCommit,
            &key,
            &serde_json::Value::Null,
        );
        tx.commit()?;
        boundary(
            &mut self.boundary_hook,
            SavePoint::InternAfterCommit,
            &key,
            &serde_json::Value::Null,
        );
        Ok(id)
    }
}

/// A reader never runs migrations or exposes the store's write operations.
pub struct Reader {
    client: Client,
    settings: tl3_config::Store,
}

impl Reader {
    pub fn reconnect(&mut self) -> Result<()> {
        *self = Self::open(&self.settings)?;
        Ok(())
    }

    pub fn ingress_cursor(&mut self, source: &str) -> Result<Option<serde_json::Value>> {
        let mut tx = self.transaction()?;
        let cursor = tx
            .query_opt(
                "SELECT cursor FROM tl3_ingress_cursors WHERE source = $1",
                &[&source],
            )?
            .map(|row| row.get(0));
        tx.commit()?;
        Ok(cursor)
    }

    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        let mut reader = Self {
            client: connect(settings, true)?,
            settings: settings.clone(),
        };
        reader.transaction()?.commit()?;
        Ok(reader)
    }

    /// Extra status queries must use this transaction to share the snapshot.
    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        let mut tx = read_transaction(&mut self.client)?;
        check_schema(&mut tx)?;
        Ok(tx)
    }

    pub fn load(&mut self) -> Result<Snapshot> {
        let mut tx = self.transaction()?;
        let snapshot = load_snapshot(&mut tx)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub fn effects(&mut self) -> Result<Vec<Effect>> {
        let mut tx = self.transaction()?;
        let effects = load_effects(&mut tx)?;
        tx.commit()?;
        Ok(effects)
    }

    pub fn events_after(&mut self, revision: Revision, limit: u32) -> Result<Vec<EventRecord>> {
        let mut tx = self.transaction()?;
        let events = load_events(&mut tx, revision, limit)?;
        tx.commit()?;
        Ok(events)
    }
}

const POSTMORTEM_RECORD: &str =
    "id, node_id, trigger, reason, sample_share, status, run_key, attempts";

fn postmortem_record(row: &Row) -> PostmortemRecord {
    PostmortemRecord {
        id: row.get(0),
        node: row.get(1),
        trigger: row.get(2),
        reason: row.get(3),
        sample_share: row.get(4),
        status: row.get(5),
        run_key: row.get(6),
        attempts: row.get(7),
    }
}

fn insert_grade(tx: &mut Transaction<'_>, grade: &CodeGrade, revision: i64) -> Result<()> {
    tx.execute("INSERT INTO tl3_code_grades (node_id, trigger, rubric_version, dimension, grade, fact, revision, recorded_at) SELECT $1, $2, $3, $4, $5, $6, revision, committed_at FROM tl3_events WHERE revision = $7 ON CONFLICT (node_id, trigger, rubric_version, dimension) DO NOTHING",
        &[&grade.node, &grade.trigger, &grade.rubric_version, &grade.dimension, &grade.grade, &grade.fact, &revision])?;
    Ok(())
}

/// #403: a runner's claim, result, failure or release. Only a claim on an
/// owed row and a finish under the claim's own run key change anything.
fn apply_postmortem(
    tx: &mut Transaction<'_>,
    update: &PostmortemUpdate,
    revision: i64,
) -> Result<()> {
    const AT: &str = "(SELECT committed_at FROM tl3_events WHERE revision = $3)";
    match update {
        PostmortemUpdate::Claim {
            id,
            run_key,
            rubric_version,
        } => {
            let claimed = tx.execute(
                &format!("UPDATE tl3_postmortems SET status = 'running', run_key = $2, started_at = {AT}, rubric_version = $4 WHERE id = $1 AND status = 'owed'"),
                &[id, run_key, &revision, rubric_version],
            )?;
            if claimed != 1 {
                return Err(Error::Invalid("the postmortem is no longer owed"));
            }
            tx.execute(
                &format!("INSERT INTO tl3_postmortem_claims (run_key, postmortem_id, revision, claimed_at) VALUES ($2, $1, $3, {AT})"),
                &[id, run_key, &revision],
            )?;
        }
        PostmortemUpdate::Done {
            id,
            run_key,
            result,
        } => {
            let done = tx.execute(
                &format!("UPDATE tl3_postmortems SET status = 'done', finished_at = {AT}, failed_at = NULL, failure = NULL, cause = $4, summary = $5, timeline = $6, process_failures = $7, grades = $8, notes = $9, assessment_ids = $10, model = $11 WHERE id = $1 AND status = 'running' AND run_key = $2"),
                &[id, run_key, &revision, &result.cause, &result.summary, &result.timeline, &result.process_failures, &result.grades, &result.notes, &result.assessment_ids, &result.model],
            )?;
            // A stale run's result changes nothing.
            if done != 1 {
                return Ok(());
            }
            claim_ended(tx, run_key, "done", None)?;
            for item in &result.recommendations {
                tx.execute("INSERT INTO tl3_postmortem_recommendations (postmortem_id, fingerprint, rank, phase, kind, target, operation, before, after, evidence, effect) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                    &[id, &item.fingerprint, &item.rank, &item.phase, &item.kind, &item.target, &item.operation, &item.before, &item.after, &item.evidence, &item.effect])?;
            }
            for grade in &result.code_grades {
                insert_grade(tx, grade, revision)?;
            }
            if let Some((operation, item)) = &result.memory {
                queue_memory_in(tx, operation, item)?;
            }
        }
        PostmortemUpdate::Failed {
            id,
            run_key,
            failure,
            judge_failure,
        } => {
            // A failure worth retrying is owed again, twice at most (#486).
            let retried = RETRIED_FAILURES.contains(&failure.as_str());
            let failure = judge_failure.map_or_else(|| failure.clone(), |cause| cause.to_string());
            let failed = tx.execute(
                &format!("UPDATE tl3_postmortems SET status = CASE WHEN $5 AND attempts < 2 THEN 'owed' ELSE 'failed' END, attempts = CASE WHEN $5 AND attempts < 2 THEN attempts + 1 ELSE attempts END, run_key = CASE WHEN $5 AND attempts < 2 THEN NULL ELSE run_key END, started_at = CASE WHEN $5 AND attempts < 2 THEN NULL ELSE started_at END, model = CASE WHEN $5 AND attempts < 2 THEN NULL ELSE model END, rubric_version = CASE WHEN $5 AND attempts < 2 THEN NULL ELSE rubric_version END, failed_at = {AT}, failure = $4 WHERE id = $1 AND status = 'running' AND run_key = $2"),
                &[id, run_key, &revision, &failure, &retried],
            )?;
            if failed == 1 {
                claim_ended(tx, run_key, "failed", Some(failure.as_str()))?;
            }
        }
        PostmortemUpdate::Release { id, run_key } => {
            let released = tx.execute(
                "UPDATE tl3_postmortems SET status = 'owed', run_key = NULL, started_at = NULL, model = NULL, rubric_version = NULL WHERE id = $1 AND status = 'running' AND run_key = $2",
                &[id, run_key],
            )?;
            if released == 1 {
                claim_ended(tx, run_key, "released", None)?;
            }
        }
    }
    Ok(())
}

/// Postmortem failure classes worth another run: the worker died or ran out
/// of time. A stall, a refused launch or an invalid reply would repeat (#486).
const RETRIED_FAILURES: [&str; 2] = ["worker_failed", "deadline"];

fn claim_ended(
    tx: &mut Transaction<'_>,
    run_key: &str,
    outcome: &str,
    failure: Option<&str>,
) -> Result<()> {
    tx.execute(
        "UPDATE tl3_postmortem_claims SET outcome = $2, failure = $3 WHERE run_key = $1",
        &[&run_key, &outcome, &failure],
    )?;
    Ok(())
}

/// Queue one Hindsight memory in `tx`, parking the oldest past the outbox's capacity.
fn queue_memory_in(
    tx: &mut Transaction<'_>,
    operation_id: &str,
    item: &serde_json::Value,
) -> Result<()> {
    tx.batch_execute("LOCK TABLE tl3_memory_outbox IN SHARE ROW EXCLUSIVE MODE")?;
    tx.execute("INSERT INTO tl3_memory_outbox (operation_id, item) VALUES ($1, $2) ON CONFLICT (operation_id) DO NOTHING", &[&operation_id, item])?;
    tx.execute("UPDATE tl3_memory_outbox SET parked_reason = 'capacity', parked_at = CURRENT_TIMESTAMP WHERE sequence IN (SELECT sequence FROM tl3_memory_outbox WHERE parked_reason IS NULL ORDER BY sequence DESC OFFSET 10000)", &[])?;
    Ok(())
}

/// Create or update a login that may read TL3's tables, now and after later
/// migrations, with the password in `password_file`. Its writes are filing
/// console commands through `tl3_console_command` and adding prompt versions
/// and moving the active prompt pointers, through `tl3_prompt_editor`.
/// Connects as the store's owner. Safe to repeat.
pub fn grant_reader(
    settings: &tl3_config::Store,
    role: &str,
    password_file: &std::path::Path,
) -> Result<()> {
    let valid = role.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && role
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(Error::Invalid("reader role must be a lowercase identifier"));
    }
    let password = fs::read(password_file).map_err(Error::Password)?;
    let password =
        String::from_utf8(password).map_err(|_| Error::Invalid("reader password must be text"))?;
    let password = password.trim_end_matches(['\n', '\r']);
    if password.is_empty() {
        return Err(Error::Invalid("reader password must not be empty"));
    }
    let mut client = connect(settings, false)?;
    let mut tx = client.transaction()?;
    // The password is in the statement text below; a failure must not log it.
    tx.batch_execute("SET LOCAL log_min_error_statement = panic")?;
    // Utility statements take no parameters, so Postgres quotes the password.
    let password: String = tx
        .query_one("SELECT quote_literal($1)", &[&password])?
        .get(0);
    let exists = tx
        .query_opt("SELECT 1 FROM pg_roles WHERE rolname = $1", &[&role])?
        .is_some();
    let verb = if exists { "ALTER" } else { "CREATE" };
    tx.batch_execute(&format!(
        "{verb} ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD {password};
         DO $grant$ BEGIN EXECUTE format('GRANT CONNECT ON DATABASE %I TO {role}', current_database()); END $grant$;
         GRANT USAGE ON SCHEMA public TO {role};
         REVOKE ALL ON ALL TABLES IN SCHEMA public FROM {role};
         GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role};
         ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO {role};
         DO $editor$ BEGIN CREATE ROLE tl3_prompt_editor NOLOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $editor$;
         GRANT tl3_prompt_editor TO {role};
         DO $editor$ BEGIN CREATE ROLE tl3_settings_editor NOLOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $editor$;
         GRANT tl3_settings_editor TO {role};
         ALTER ROLE {role} SET default_transaction_read_only = on;
         DO $grant$ BEGIN
             IF to_regprocedure('tl3_console_command(text, bigint, jsonb)') IS NOT NULL THEN
                 GRANT EXECUTE ON FUNCTION tl3_console_command(text, bigint, jsonb) TO {role};
             END IF;
         END $grant$;"
    ))?;
    tx.commit()?;
    Ok(())
}

/// A read-only session that waits for committed changes on the change feed.
/// It blocks on the socket, so a waiting reader never polls.
pub struct Listener {
    client: Client,
}

impl Listener {
    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        Self::listen(settings, "LISTEN tl3_changes")
    }

    /// tl3d's wake for ingress filed outside its own process, such as console commands.
    pub fn ingress(settings: &tl3_config::Store) -> Result<Self> {
        Self::listen(settings, "LISTEN tl3_ingress")
    }

    fn listen(settings: &tl3_config::Store, statement: &str) -> Result<Self> {
        let mut client = listen_config(settings)?.connect(NoTls)?;
        client.batch_execute(statement)?;
        Ok(Self { client })
    }

    /// Block until at least one change commits, then take every notification
    /// already received. Returns the tables that changed. An error means the
    /// session ended, and changes after it may be missed until a new listener opens.
    pub fn wait(&mut self) -> Result<std::collections::BTreeSet<String>> {
        use postgres::fallible_iterator::FallibleIterator;
        let mut notifications = self.client.notifications();
        let Some(first) = notifications.blocking_iter().next()? else {
            return Err(Error::Invalid("change feed session closed"));
        };
        let mut tables = std::collections::BTreeSet::from([first.payload().to_string()]);
        let mut pending = notifications.iter();
        while let Some(next) = pending.next()? {
            tables.insert(next.payload().to_string());
        }
        Ok(tables)
    }
}

/// The console's session for filing commands. Its login may only read, so the
/// one write goes through `tl3_console_command` in an explicit read-write
/// transaction.
pub struct ConsoleCommands {
    client: Client,
}

impl ConsoleCommands {
    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        Ok(Self {
            client: connect(settings, false)?,
        })
    }

    /// File a command for tl3d. False when this id was already filed.
    pub fn submit(&mut self, id: &str, node: u64, action: &serde_json::Value) -> Result<bool> {
        let node = i64::try_from(node).map_err(|_| Error::Invalid("node id out of range"))?;
        let mut tx = self.client.build_transaction().read_only(false).start()?;
        let filed = tx
            .query_one(
                "SELECT tl3_console_command($1, $2, $3)",
                &[&id, &node, action],
            )?
            .get(0);
        tx.commit()?;
        Ok(filed)
    }
}

fn load_effects(client: &mut impl postgres::GenericClient) -> Result<Vec<Effect>> {
    client.query("SELECT effect_key, epoch, kind, payload, state::text, created_revision, updated_revision FROM tl3_effects ORDER BY created_revision, effect_key", &[])?
            .into_iter().map(decode_effect).collect()
}

fn load_events(
    client: &mut impl postgres::GenericClient,
    revision: Revision,
    limit: u32,
) -> Result<Vec<EventRecord>> {
    client.query("SELECT event_key, payload, revision, epoch, result FROM tl3_events WHERE revision > $1 ORDER BY revision LIMIT $2", &[&revision.get(), &i64::from(limit)])?
            .into_iter().map(|row| Ok(EventRecord {
                key: row.get(0), payload: row.get(1),
                receipt: CommitReceipt { revision: Revision::try_from(row.get::<_, i64>(2))?, epoch: Epoch::try_from(row.get::<_, i64>(3))?, result: row.get(4) },
            })).collect()
}

fn check_schema(tx: &mut Transaction<'_>) -> Result<()> {
    if tx
        .query_one("SELECT to_regclass('tl3_schema_migrations') IS NULL", &[])?
        .get::<_, bool>(0)
    {
        return Err(Error::NotInitialized);
    }
    let applied = tx.query(
        "SELECT version, sql FROM tl3_schema_migrations ORDER BY version",
        &[],
    )?;
    let Some(last) = applied.last() else {
        return Err(Error::NotInitialized);
    };
    let actual = last.get::<_, i32>(0);
    let expected = MIGRATIONS.last().expect("store migration").0;
    if actual != expected {
        return Err(Error::SchemaVersion { actual, expected });
    }
    if applied.len() != MIGRATIONS.len()
        || applied
            .iter()
            .zip(MIGRATIONS)
            .any(|(row, &(version, sql))| {
                row.get::<_, i32>(0) != version || row.get::<_, &str>(1) != sql
            })
    {
        return Err(Error::MigrationMismatch);
    }
    for table in [
        "tl3_meta",
        "tl3_world",
        "tl3_events",
        "tl3_effects",
        "tl3_credit",
        "tl3_intern",
    ] {
        if tx
            .query_one("SELECT to_regclass($1) IS NULL", &[&table])?
            .get::<_, bool>(0)
        {
            return Err(Error::NotInitialized);
        }
    }
    Ok(())
}

const LISTEN_KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const LISTEN_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const LISTEN_KEEPALIVE_RETRIES: u32 = 3;

/// A LISTEN session idles for hours. Over TCP, keepalives find a dead peer
/// within 60 + 3 x 10 seconds, so the session ends and its owner reconnects
/// and drains, instead of waiting on the kernel's default of minutes. libpq
/// ignores them on a Unix socket, which closes when the server dies anyway.
fn listen_config(settings: &tl3_config::Store) -> Result<postgres::Config> {
    let mut config = config(settings, true)?;
    config
        .keepalives(true)
        .keepalives_idle(LISTEN_KEEPALIVE_IDLE)
        .keepalives_interval(LISTEN_KEEPALIVE_INTERVAL)
        .keepalives_retries(LISTEN_KEEPALIVE_RETRIES);
    Ok(config)
}

fn connect(settings: &tl3_config::Store, read_only: bool) -> Result<Client> {
    Ok(config(settings, read_only)?.connect(NoTls)?)
}

fn config(settings: &tl3_config::Store, read_only: bool) -> Result<postgres::Config> {
    settings.validate()?;
    let mut connection = postgres::Config::new();
    connection
        .host(&settings.host)
        .port(settings.port)
        .dbname(&settings.database)
        .user(&settings.user)
        .connect_timeout(Duration::from_secs(settings.connect_timeout_secs.into()));
    if let Some(path) = &settings.password_file {
        let password = fs::read(path).map_err(Error::Password)?;
        let password = password.strip_suffix(b"\n").unwrap_or(&password);
        let password = password.strip_suffix(b"\r").unwrap_or(password);
        connection.password(password);
    }
    if read_only {
        connection.options("-c default_transaction_read_only=on");
    }
    Ok(connection)
}

fn read_transaction(client: &mut Client) -> Result<Transaction<'_>> {
    Ok(client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()?)
}

/// Load the controller snapshot inside the caller's existing read transaction.
pub fn load_snapshot(tx: &mut Transaction<'_>) -> Result<Snapshot> {
    let (revision, epoch) = metadata(tx, false)?;
    let rows = tx
        .query(
            "SELECT table_name, row_id, value, version, arrival FROM tl3_world ORDER BY table_name, arrival",
            &[],
        )?
        .into_iter()
        .map(|row| {
            Ok(WorldRow {
                table: row.get(0),
                id: row.get(1),
                value: row.get(2),
                version: Revision::try_from(row.get::<_, i64>(3))?,
                arrival: row.get(4),
            })
        })
        .collect::<Result<_>>()?;
    let pending_effects = tx.query("SELECT effect_key, epoch, kind, payload, state::text, created_revision, updated_revision FROM tl3_effects WHERE state IN ('Intended', 'Sent', 'Uncertain', 'Cancelling') ORDER BY created_revision, effect_key", &[])?
            .into_iter().map(decode_effect).collect::<Result<_>>()?;
    let credit = tx.query("SELECT node_id, reason, root_id, amount, revision FROM tl3_credit ORDER BY node_id, reason", &[])?
            .into_iter().map(|row| Ok(CreditRecord {
                entry: CreditEntry { node: row.get(0), reason: row.get(1), root: row.get(2), amount: row.get(3) },
                revision: Revision::try_from(row.get::<_, i64>(4))?,
            })).collect::<Result<_>>()?;
    let interned = tx
        .query(
            "SELECT id, kind::text, scope, value FROM tl3_intern ORDER BY id",
            &[],
        )?
        .into_iter()
        .map(|row| {
            Ok(Interned {
                id: row.get(0),
                kind: InternKind::try_from(row.get::<_, &str>(1))?,
                scope: row.get(2),
                value: row.get(3),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Snapshot {
        revision,
        epoch,
        rows,
        pending_effects,
        credit,
        interned,
    })
}

fn metadata(tx: &mut Transaction<'_>, lock: bool) -> Result<(Revision, Epoch)> {
    let sql = if lock {
        "SELECT revision, epoch FROM tl3_meta WHERE singleton FOR UPDATE"
    } else {
        "SELECT revision, epoch FROM tl3_meta WHERE singleton"
    };
    let row = tx.query_one(sql, &[])?;
    Ok((
        Revision::try_from(row.get::<_, i64>(0))?,
        Epoch::try_from(row.get::<_, i64>(1))?,
    ))
}

/// #371: a later release may add object members to the payload of the same
/// fact. A null member reads as absent. Any changed or dropped value is a
/// different fact.
fn same_fact(stored: &serde_json::Value, new: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (stored, new) {
        (Value::Object(stored), Value::Object(new)) => stored
            .iter()
            .filter(|(_, value)| !value.is_null())
            .all(|(key, value)| new.get(key).is_some_and(|new| same_fact(value, new))),
        (Value::Array(stored), Value::Array(new)) => {
            stored.len() == new.len() && stored.iter().zip(new).all(|(s, n)| same_fact(s, n))
        }
        // JSONB compares numbers by value; a stored 1.0e18 may read back as an
        // integer. Two integers compare exactly, so values above 2^53 stay distinct.
        (Value::Number(stored), Value::Number(new)) => {
            stored == new
                || ((stored.is_f64() || new.is_f64())
                    && stored.as_f64().is_some_and(|s| Some(s) == new.as_f64()))
        }
        _ => stored == new,
    }
}

fn receipt(row: Row) -> Result<CommitReceipt> {
    Ok(CommitReceipt {
        revision: Revision::try_from(row.get::<_, i64>(0))?,
        epoch: Epoch::try_from(row.get::<_, i64>(1))?,
        result: row.get(2),
    })
}

fn decode_effect(row: Row) -> Result<Effect> {
    Ok(Effect {
        key: row.get(0),
        epoch: Epoch::try_from(row.get::<_, i64>(1))?,
        kind: row.get(2),
        payload: row.get(3),
        state: EffectState::try_from(row.get::<_, &str>(4))?,
        created_revision: Revision::try_from(row.get::<_, i64>(5))?,
        updated_revision: Revision::try_from(row.get::<_, i64>(6))?,
    })
}

#[cfg(test)]
mod same_fact_tests {
    use super::same_fact;
    use serde_json::json;

    #[test]
    fn integers_compare_exactly_and_floats_by_value() {
        let big = 1_u64 << 53;
        assert!(!same_fact(&json!(big), &json!(big + 1)));
        assert!(!same_fact(
            &json!({"reset_ms": big}),
            &json!({"reset_ms": big + 1})
        ));
        assert!(same_fact(
            &json!(1_000_000_000_000_000_000_u64),
            &json!(1.0e18)
        ));
        assert!(same_fact(
            &json!(1.0e18),
            &json!(1_000_000_000_000_000_000_u64)
        ));
        assert!(!same_fact(&json!(1), &json!(2.0)));
    }

    #[test]
    fn later_members_extend_the_same_fact() {
        let stored = json!({"WorkerReply": {"generation": 3, "reply": {"kind": "done"}}});
        assert!(same_fact(
            &stored,
            &json!({"WorkerReply": {"generation": 3, "reply": {"kind": "done"}, "model": "m"}})
        ));
        assert!(same_fact(&json!({"a": 1, "model": null}), &json!({"a": 1})));
        assert!(!same_fact(&json!({"a": 1, "model": "m"}), &json!({"a": 1})));
        assert!(!same_fact(&json!({"a": [1, 2]}), &json!({"a": [1, 2, 3]})));
        assert!(!same_fact(
            &stored,
            &json!({"WorkerReply": {"generation": 4, "reply": {"kind": "done"}}})
        ));
    }
}

#[cfg(test)]
mod listen_tests {
    use super::*;

    /// The kernel's defaults would take minutes to give up on a dead peer;
    /// the session names its own probe interval and count.
    #[test]
    fn listen_sessions_find_a_dead_peer_within_ninety_seconds() {
        let config = listen_config(&tl3_config::Store::default()).unwrap();
        assert_eq!(config.get_keepalives_idle(), Duration::from_secs(60));
        assert_eq!(
            config.get_keepalives_interval(),
            Some(Duration::from_secs(10))
        );
        assert_eq!(config.get_keepalives_retries(), Some(3));
        assert!(config.get_keepalives());
    }
}
