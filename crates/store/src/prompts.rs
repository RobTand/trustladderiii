//! Role prompt versions (PRD #327, C5). tl3d seeds the built-in prompts and
//! records which version each worker run uses; the console adds versions and
//! moves each role's active pointer. None of this is world state.

use crate::{Error, Result, Store, connect, read_transaction};
use postgres::{Client, GenericClient, IsolationLevel, Row, Transaction};

/// Who added a built-in version, in the version history.
pub const BUILTIN_AUTHOR: &str = "TL3 deploy";
/// The longest prompt text, in bytes. Migration 12 holds the same limit.
pub const TEXT_LIMIT: usize = 256 * 1024;
/// The longest version note, in characters. Migration 12 holds the same limit.
pub const NOTE_LIMIT: usize = 500;
const SEED_LOCK: i64 = 0x544c3350524f4d50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptVersion {
    pub id: i64,
    /// 1 for a role's first version, counting up.
    pub number: i64,
    pub role: String,
    pub body: String,
    pub builtin: bool,
    pub author: String,
    pub note: String,
    pub created_ms: i64,
}

/// A version in a role's history, without its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSummary {
    pub id: i64,
    pub number: i64,
    pub builtin: bool,
    pub author: String,
    pub note: String,
    pub created_ms: i64,
}

/// Which version of a role runs, and its history, newest first. Only the
/// active version and a waiting built-in update carry their text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolePrompts {
    pub role: String,
    pub active: PromptVersion,
    pub activated_by: String,
    pub activated_ms: i64,
    /// A built-in version a deploy shipped while another version stayed active,
    /// until Rob activates it or keeps his own.
    pub builtin_update: Option<PromptVersion>,
    pub versions: Vec<PromptSummary>,
}

/// Why the console's edit changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    UnknownRole,
    UnknownVersion,
    /// The edit started from a version that is no longer active.
    Stale {
        active: i64,
    },
    EmptyText,
    EmptyNote,
    /// Longer than `TEXT_LIMIT` bytes.
    TextTooLong,
    /// Longer than `NOTE_LIMIT` characters.
    NoteTooLong,
    /// Nothing to keep: no built-in update waits for this version.
    NoUpdate,
}

pub type Edit = std::result::Result<RolePrompts, Refusal>;

impl Store {
    /// Add each changed built-in prompt as a new version. It becomes active only
    /// where the previous built-in version was active, so a deploy never
    /// replaces a version Rob chose. Safe to repeat.
    pub fn seed_prompts(&mut self, builtins: &[(&str, &str)]) -> Result<()> {
        self.seed_prompts_checked(builtins, || Ok(()))
    }

    /// #785: check admission before each startup prompt write and commit.
    pub fn seed_prompts_owned(
        &mut self,
        builtins: &[(&str, &str)],
        ownership: &mut crate::Ownership,
    ) -> Result<()> {
        self.seed_prompts_checked(builtins, || ownership.ensure_held())
    }

    fn seed_prompts_checked(
        &mut self,
        builtins: &[(&str, &str)],
        mut before_write: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        before_write()?;
        let mut tx = self.client.transaction()?;
        tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&SEED_LOCK])?;
        for &(role, body) in builtins {
            let latest = tx.query_opt(
                "SELECT id, body FROM tl3_prompt_versions WHERE role = $1 AND builtin ORDER BY id DESC LIMIT 1",
                &[&role],
            )?;
            if latest
                .as_ref()
                .is_some_and(|row| row.get::<_, &str>(1) == body)
            {
                continue;
            }
            let note = if latest.is_some() {
                "A deploy shipped a changed built-in prompt."
            } else {
                "The built-in prompt TL3 shipped with."
            };
            before_write()?;
            let id: i64 = tx
                .query_one(
                    "INSERT INTO tl3_prompt_versions (role, body, builtin, author, note) VALUES ($1, $2, true, $3, $4) RETURNING id",
                    &[&role, &body, &BUILTIN_AUTHOR, &note],
                )?
                .get(0);
            before_write()?;
            match latest {
                None => tx.execute(
                    "INSERT INTO tl3_prompt_active (role, version, builtin_seen, activated_by) VALUES ($1, $2, $2, $3) ON CONFLICT (role) DO NOTHING",
                    &[&role, &id, &BUILTIN_AUTHOR],
                )?,
                Some(previous) => tx.execute(
                    "UPDATE tl3_prompt_active SET version = $2, builtin_seen = $2, activated_by = $3, activated_at = clock_timestamp() WHERE role = $1 AND version = $4",
                    &[&role, &id, &BUILTIN_AUTHOR, &previous.get::<_, i64>(0)],
                )?,
            };
        }
        before_write()?;
        tx.commit()?;
        Ok(())
    }

    /// The prompt version a worker run uses, recorded with the run. The first
    /// call for a run takes the active version; later attempts of the same run
    /// keep it. None for a role with no versions.
    pub fn run_prompt(&mut self, run_key: &str, role: &str) -> Result<Option<PromptVersion>> {
        let mut tx = self.client.transaction()?;
        tx.execute(
            "INSERT INTO tl3_prompt_runs (run_key, role, version) SELECT $1, role, version FROM tl3_prompt_active WHERE role = $2 ON CONFLICT (run_key, role) DO NOTHING",
            &[&run_key, &role],
        )?;
        let version = tx
            .query_opt(
                &format!(
                    "{VERSION} WHERE v.id = (SELECT version FROM tl3_prompt_runs WHERE run_key = $1 AND role = $2)"
                ),
                &[&run_key, &role],
            )?
            .map(version);
        tx.commit()?;
        Ok(version)
    }
}

/// The console's session for prompt versions. Its login may read every TL3
/// table but write only the prompt versions and the active pointers. It checks
/// only that the prompt tables exist: the console judges the store's
/// migrations itself and serves a store ahead of the TL3 it pins.
pub struct Prompts {
    client: Client,
}

impl Prompts {
    pub fn open(settings: &tl3_config::Store) -> Result<Self> {
        let mut prompts = Self {
            client: connect(settings, false)?,
        };
        prompts.read()?.commit()?;
        Ok(prompts)
    }

    /// Every role with a version, by name.
    pub fn roles(&mut self) -> Result<Vec<RolePrompts>> {
        let mut tx = self.read()?;
        let roles: Vec<String> = tx
            .query("SELECT role FROM tl3_prompt_active ORDER BY role", &[])?
            .iter()
            .map(|row| row.get(0))
            .collect();
        let mut all = Vec::with_capacity(roles.len());
        for role in roles {
            all.extend(load(&mut tx, &role)?);
        }
        tx.commit()?;
        Ok(all)
    }

    pub fn role(&mut self, role: &str) -> Result<Option<RolePrompts>> {
        let mut tx = self.read()?;
        let prompts = load(&mut tx, role)?;
        tx.commit()?;
        Ok(prompts)
    }

    /// One version of a role, with its text.
    pub fn version(&mut self, role: &str, id: i64) -> Result<Option<PromptVersion>> {
        let mut tx = self.read()?;
        let found = full(&mut tx, role, id)?;
        tx.commit()?;
        Ok(found)
    }

    /// Whether the session still reaches Postgres. Check before an edit, so a
    /// dropped session is replaced before anything is sent rather than after.
    pub fn is_live(&mut self) -> bool {
        self.client
            .is_valid(std::time::Duration::from_secs(5))
            .is_ok()
    }

    /// Add a version and make it active at once. `base` is the active version
    /// the edit started from; if another became active since, nothing changes.
    pub fn save(
        &mut self,
        role: &str,
        body: &str,
        note: &str,
        author: &str,
        base: i64,
    ) -> Result<Edit> {
        if body.trim().is_empty() {
            return Ok(Err(Refusal::EmptyText));
        }
        if note.trim().is_empty() {
            return Ok(Err(Refusal::EmptyNote));
        }
        if body.len() > TEXT_LIMIT {
            return Ok(Err(Refusal::TextTooLong));
        }
        if note.trim().chars().count() > NOTE_LIMIT {
            return Ok(Err(Refusal::NoteTooLong));
        }
        self.edit(role, |tx, active| {
            if active != base {
                return Ok(Err(Refusal::Stale { active }));
            }
            let id: i64 = tx
                .query_one(
                    "INSERT INTO tl3_prompt_versions (role, body, author, note) VALUES ($1, $2, $3, $4) RETURNING id",
                    &[&role, &body, &author, &note.trim()],
                )?
                .get(0);
            point(tx, role, "version = $2", id, author)?;
            Ok(Ok(()))
        })
    }

    /// Make an existing version active, as a revert does. `expected` is the
    /// version the caller saw active; if another became active since, nothing
    /// changes.
    pub fn activate(
        &mut self,
        role: &str,
        version: i64,
        author: &str,
        expected: i64,
    ) -> Result<Edit> {
        self.edit(role, |tx, active| {
            if active != expected {
                return Ok(Err(Refusal::Stale { active }));
            }
            let builtin = tx
                .query_opt(
                    "SELECT builtin FROM tl3_prompt_versions WHERE role = $1 AND id = $2",
                    &[&role, &version],
                )?
                .map(|row| row.get::<_, bool>(0));
            match builtin {
                None => Ok(Err(Refusal::UnknownVersion)),
                Some(_) if version == active => Ok(Ok(())),
                // Choosing a built-in version decides every older built-in update.
                Some(true) => point(
                    tx,
                    role,
                    "version = $2, builtin_seen = GREATEST(builtin_seen, $2)",
                    version,
                    author,
                )
                .map(Ok),
                Some(false) => point(tx, role, "version = $2", version, author).map(Ok),
            }
        })
    }

    /// Keep the active version over built-in update `builtin`.
    pub fn keep(&mut self, role: &str, builtin: i64) -> Result<Edit> {
        self.edit(role, |tx, _| {
            let Some(prompts) = load(tx, role)? else {
                return Ok(Err(Refusal::UnknownRole));
            };
            if prompts.builtin_update.map(|update| update.id) != Some(builtin) {
                return Ok(Err(Refusal::NoUpdate));
            }
            tx.execute(
                "UPDATE tl3_prompt_active SET builtin_seen = $2 WHERE role = $1",
                &[&role, &builtin],
            )?;
            Ok(Ok(()))
        })
    }

    /// Run one edit with the role's pointer locked, then read the result.
    fn edit(
        &mut self,
        role: &str,
        change: impl FnOnce(&mut Transaction<'_>, i64) -> Result<std::result::Result<(), Refusal>>,
    ) -> Result<Edit> {
        let mut tx = self
            .client
            .build_transaction()
            .isolation_level(IsolationLevel::ReadCommitted)
            .read_only(false)
            .start()?;
        prompt_tables(&mut tx)?;
        let Some(active) = tx.query_opt(
            "SELECT version FROM tl3_prompt_active WHERE role = $1 FOR UPDATE",
            &[&role],
        )?
        else {
            return Ok(Err(Refusal::UnknownRole));
        };
        if let Err(refusal) = change(&mut tx, active.get(0))? {
            return Ok(Err(refusal));
        }
        let prompts = load(&mut tx, role)?.ok_or(Error::Invalid("prompt role vanished"))?;
        tx.commit()?;
        Ok(Ok(prompts))
    }

    fn read(&mut self) -> Result<Transaction<'_>> {
        let mut tx = read_transaction(&mut self.client)?;
        prompt_tables(&mut tx)?;
        Ok(tx)
    }
}

fn prompt_tables(tx: &mut Transaction<'_>) -> Result<()> {
    let present: bool = tx
        .query_one(
            "SELECT to_regclass('tl3_prompt_versions') IS NOT NULL AND to_regclass('tl3_prompt_active') IS NOT NULL",
            &[],
        )?
        .get(0);
    if present {
        let actual: Option<i32> = tx
            .query_one("SELECT MAX(version) FROM tl3_schema_migrations", &[])?
            .get(0);
        let expected = crate::MIGRATIONS.last().expect("store migration").0;
        if actual.unwrap_or(0) < expected {
            return Err(Error::SchemaVersion {
                actual: actual.unwrap_or(0),
                expected,
            });
        }
        Ok(())
    } else {
        Err(Error::NotInitialized)
    }
}

fn point(
    tx: &mut Transaction<'_>,
    role: &str,
    set: &str,
    version: i64,
    author: &str,
) -> Result<()> {
    tx.execute(
        &format!(
            "UPDATE tl3_prompt_active SET {set}, activated_by = $3, activated_at = clock_timestamp() WHERE role = $1"
        ),
        &[&role, &version, &author],
    )?;
    Ok(())
}

const VERSION: &str = "SELECT v.id, (SELECT count(*) FROM tl3_prompt_versions o WHERE o.role = v.role AND o.id <= v.id), v.role, v.body, v.builtin, v.author, v.note, (extract(epoch FROM v.created_at) * 1000)::bigint FROM tl3_prompt_versions v";

fn version(row: Row) -> PromptVersion {
    PromptVersion {
        id: row.get(0),
        number: row.get(1),
        role: row.get(2),
        body: row.get(3),
        builtin: row.get(4),
        author: row.get(5),
        note: row.get(6),
        created_ms: row.get(7),
    }
}

fn full(client: &mut impl GenericClient, role: &str, id: i64) -> Result<Option<PromptVersion>> {
    Ok(client
        .query_opt(
            &format!("{VERSION} WHERE v.role = $1 AND v.id = $2"),
            &[&role, &id],
        )?
        .map(version))
}

fn load(client: &mut impl GenericClient, role: &str) -> Result<Option<RolePrompts>> {
    let Some(pointer) = client.query_opt(
        "SELECT version, builtin_seen, activated_by, (extract(epoch FROM activated_at) * 1000)::bigint FROM tl3_prompt_active WHERE role = $1",
        &[&role],
    )?
    else {
        return Ok(None);
    };
    let versions: Vec<PromptSummary> = client
        .query(
            "SELECT id, row_number() OVER (ORDER BY id), builtin, author, note, (extract(epoch FROM created_at) * 1000)::bigint FROM tl3_prompt_versions WHERE role = $1 ORDER BY id DESC",
            &[&role],
        )?
        .into_iter()
        .map(|row| PromptSummary {
            id: row.get(0),
            number: row.get(1),
            builtin: row.get(2),
            author: row.get(3),
            note: row.get(4),
            created_ms: row.get(5),
        })
        .collect();
    let active: i64 = pointer.get(0);
    let seen: i64 = pointer.get(1);
    let update = versions
        .iter()
        .find(|version| version.builtin)
        .map(|latest| latest.id)
        .filter(|&latest| latest > seen && latest != active);
    let builtin_update = match update {
        Some(id) => full(client, role, id)?,
        None => None,
    };
    Ok(Some(RolePrompts {
        role: role.into(),
        active: full(client, role, active)?
            .ok_or(Error::Invalid("active prompt version is missing"))?,
        activated_by: pointer.get(2),
        activated_ms: pointer.get(3),
        builtin_update,
        versions,
    }))
}
