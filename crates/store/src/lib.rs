#![forbid(unsafe_code)]

mod types;
pub use types::*;

use postgres::{Client, IsolationLevel, NoTls, Row};
pub use postgres::{Error as DatabaseError, Transaction};
use std::{fs, time::Duration};

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
    #[error("event key already has a different payload")]
    EventKeyReused,
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
}

const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/0001_store.sql")),
    (2, include_str!("../migrations/0002_world_arrival.sql")),
    (3, include_str!("../migrations/0003_workers.sql")),
];

impl Store {
    /// Uses only the supplied settings. No environment or implicit credential file is read.
    /// Transport is local TCP or a Unix socket, without TLS.
    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        let mut store = Self {
            client: connect(settings, false)?,
        };
        if store.client.query_one("SHOW fsync", &[])?.get::<_, &str>(0) != "on" {
            return Err(Error::Invalid("store requires Postgres fsync = on"));
        }
        store.client.batch_execute("SET synchronous_commit = on")?;
        store.migrate()?;
        Ok(store)
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
            "SELECT revision, epoch, result, payload = $2 FROM tl3_events WHERE event_key = $1",
            &[&change.event_key, &change.event],
        )? {
            if !row.get::<_, bool>(3) {
                return Err(Error::EventKeyReused);
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
        for row in &change.rows {
            match row {
                RowChange::Put { table, id, value } => {
                    tx.execute("INSERT INTO tl3_world (table_name, row_id, value, version) VALUES ($1, $2, $3, $4) ON CONFLICT (table_name, row_id) DO UPDATE SET value = EXCLUDED.value, version = EXCLUDED.version",
                        &[table, id, value, &revision.get()])?;
                }
                RowChange::Delete { table, id } => {
                    tx.execute(
                        "DELETE FROM tl3_world WHERE table_name = $1 AND row_id = $2",
                        &[table, id],
                    )?;
                }
            }
        }
        for effect in &change.effects {
            tx.execute("INSERT INTO tl3_effects (effect_key, epoch, kind, payload, created_revision, updated_revision) VALUES ($1, $2, $3, $4, $5, $5)",
                &[&effect.key, &next_epoch.get(), &effect.kind, &effect.payload, &revision.get()])?;
        }
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
        tx.commit()?;
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
        if !matches!(document, "briefs" | "goals" | "audits") {
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

    /// Claim time exists even if the worker never emits progress.
    pub fn claim_worker(&mut self, key: &str, node: &str, epoch: i64, attempt: i64) -> Result<()> {
        self.client.execute("INSERT INTO tl3_worker_progress (run_key, node_id, epoch, attempt, output_offset, unit, started_ms) SELECT $1, $2, $3, $4, 0, '', (extract(epoch FROM clock_timestamp()) * 1000)::bigint FROM tl3_meta WHERE epoch = $3 ON CONFLICT (run_key) DO UPDATE SET attempt = EXCLUDED.attempt, output_offset = 0, unit = '', started_ms = EXCLUDED.started_ms, last_progress_ms = NULL WHERE tl3_worker_progress.epoch = EXCLUDED.epoch AND tl3_worker_progress.attempt < EXCLUDED.attempt", &[&key, &node, &epoch, &attempt])?;
        Ok(())
    }

    /// Replaying an output prefix never refreshes its progress time.
    pub fn record_worker_progress(
        &mut self,
        key: &str,
        attempt: i64,
        offset: i64,
        unit: &str,
    ) -> Result<()> {
        self.client.execute("UPDATE tl3_worker_progress SET output_offset = $3, unit = $4, last_progress_ms = (extract(epoch FROM clock_timestamp()) * 1000)::bigint WHERE run_key = $1 AND attempt = $2 AND output_offset < $3 AND epoch = (SELECT epoch FROM tl3_meta)", &[&key, &attempt, &offset, &unit])?;
        Ok(())
    }

    pub fn worker_progress(&mut self, node: Option<&str>) -> Result<Vec<WorkerProgress>> {
        Ok(self.client.query("SELECT run_key, node_id, epoch, attempt, output_offset, unit, started_ms, last_progress_ms FROM tl3_worker_progress WHERE ($1::text IS NULL OR node_id = $1) ORDER BY started_ms DESC, run_key", &[&node])?.into_iter().map(|r| WorkerProgress {
            key: r.get(0), node: r.get(1), epoch: r.get(2), attempt: r.get(3), output_offset: r.get(4), unit: r.get(5), started_ms: r.get(6), last_progress_ms: r.get(7),
        }).collect())
    }

    /// Only watched worker deliveries, without loading historical world rows.
    pub fn worker_watches(&mut self) -> Result<Vec<(String, serde_json::Value)>> {
        Ok(self.client.query("SELECT w.row_id, w.value FROM tl3_world w JOIN tl3_effects e ON e.effect_key = w.row_id WHERE w.table_name = 'watches' AND e.kind = 'Workers'", &[])?
            .into_iter().map(|row| (row.get(0), row.get(1))).collect())
    }

    /// A stale notification cannot revive a terminal, unwatched delivery.
    pub fn worker_reconcilable(&mut self, key: &str) -> Result<bool> {
        Ok(self.client.query_one("SELECT NOT EXISTS (SELECT 1 FROM tl3_effects WHERE effect_key = $1 AND state IN ('Confirmed', 'Failed', 'Cancelled')) OR EXISTS (SELECT 1 FROM tl3_world WHERE table_name = 'watches' AND row_id = $1)", &[&key])?.get(0))
    }

    /// Terminal effects cannot be dispatched again. Watches retain their evidence.
    pub fn retired_workers(&mut self) -> Result<Vec<String>> {
        Ok(self.client.query("SELECT p.run_key FROM tl3_worker_progress p JOIN tl3_effects e ON e.effect_key = p.run_key WHERE e.state IN ('Confirmed', 'Failed', 'Cancelled') AND NOT EXISTS (SELECT 1 FROM tl3_world w WHERE w.table_name = 'watches' AND w.row_id = p.run_key)", &[])?
            .into_iter().map(|row| row.get(0)).collect())
    }

    /// Call only after durable removal of the run directory.
    pub fn forget_worker(&mut self, key: &str) -> Result<()> {
        self.client.execute("DELETE FROM tl3_worker_progress p USING tl3_effects e WHERE p.run_key = $1 AND e.effect_key = p.run_key AND e.state IN ('Confirmed', 'Failed', 'Cancelled') AND NOT EXISTS (SELECT 1 FROM tl3_world w WHERE w.table_name = 'watches' AND w.row_id = p.run_key)", &[&key])?;
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
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid(
                "commit SHA must be 40 or 64 hexadecimal digits",
            ));
        }
        Ok(CommitId(self.intern(
            InternKind::Commit,
            node,
            &sha.to_ascii_lowercase(),
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
        if let Some(row) = self.client.query_opt("INSERT INTO tl3_intern (kind, scope, value) VALUES ($1::text::tl3_intern_kind, $2, $3) ON CONFLICT (kind, scope, value) DO NOTHING RETURNING id", params)? {
            return Ok(row.get(0));
        }
        let row = self.client.query_one(select, params)?;
        Ok(row.get(0))
    }
}

/// A reader never runs migrations or exposes the store's write operations.
pub struct Reader {
    client: Client,
}

impl Reader {
    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        let mut reader = Self {
            client: connect(settings, true)?,
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

fn connect(settings: &tl3_config::Store, read_only: bool) -> Result<Client> {
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
    Ok(connection.connect(NoTls)?)
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
