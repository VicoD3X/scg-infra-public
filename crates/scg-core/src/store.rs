use std::{
    fs,
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{CoreError, CoreResult, RealmId, RuntimePhase};

const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone)]
pub(crate) struct StoredSnapshot {
    pub revision: u64,
    pub state: Value,
    pub checksum: String,
    pub committed_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredEvent {
    pub sequence: u64,
    pub revision: u64,
    pub kind: String,
    pub occurred_at: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredCommit {
    pub snapshot: StoredSnapshot,
    pub event: StoredEvent,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeCheckpoint {
    pub clean_shutdown: bool,
}

pub(crate) struct SqliteStore {
    connection: Mutex<Connection>,
    realm: RealmId,
    snapshot_retention: usize,
    event_retention: usize,
}

impl SqliteStore {
    pub fn open(
        path: &Path,
        realm: RealmId,
        snapshot_retention: usize,
        event_retention: usize,
    ) -> CoreResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| CoreError::Storage(error.to_string()))?;
        }

        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS realm_metadata (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                realm_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS snapshots (
                revision INTEGER PRIMARY KEY,
                operation TEXT NOT NULL,
                state_json TEXT NOT NULL,
                checksum TEXT NOT NULL,
                committed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS events (
                sequence INTEGER PRIMARY KEY,
                revision INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                occurred_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS runtime_checkpoint (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                phase TEXT NOT NULL,
                last_revision INTEGER NOT NULL,
                clean_shutdown INTEGER NOT NULL,
                updated_at TEXT NOT NULL
            );
            ",
        )?;

        let expected = realm.storage_key();
        let actual: Option<String> = connection
            .query_row(
                "SELECT realm_id FROM realm_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match actual {
            Some(actual) if actual != expected => {
                return Err(CoreError::RealmMismatch { expected, actual });
            }
            Some(_) => {}
            None => {
                connection.execute(
                    "INSERT INTO realm_metadata (singleton, realm_id, schema_version, created_at)
                     VALUES (1, ?1, ?2, ?3)",
                    params![expected, SCHEMA_VERSION, now()],
                )?;
            }
        }

        Ok(Self {
            connection: Mutex::new(connection),
            realm,
            snapshot_retention: snapshot_retention.max(1),
            event_retention: event_retention.max(1),
        })
    }

    pub fn latest_snapshot(&self) -> CoreResult<Option<StoredSnapshot>> {
        let connection = self.lock()?;
        load_latest(&connection)
    }

    pub fn checkpoint(&self) -> CoreResult<Option<RuntimeCheckpoint>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT clean_shutdown FROM runtime_checkpoint WHERE singleton = 1",
                [],
                |row| {
                    let clean: i64 = row.get(0)?;
                    Ok(RuntimeCheckpoint {
                        clean_shutdown: clean != 0,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn mark_checkpoint(
        &self,
        phase: RuntimePhase,
        revision: u64,
        clean_shutdown: bool,
    ) -> CoreResult<()> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO runtime_checkpoint
                (singleton, phase, last_revision, clean_shutdown, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(singleton) DO UPDATE SET
                phase = excluded.phase,
                last_revision = excluded.last_revision,
                clean_shutdown = excluded.clean_shutdown,
                updated_at = excluded.updated_at",
            params![
                format!("{phase:?}"),
                revision,
                if clean_shutdown { 1_i64 } else { 0_i64 },
                now()
            ],
        )?;
        Ok(())
    }

    pub fn commit_state(
        &self,
        expected_revision: u64,
        operation: &str,
        state: Value,
    ) -> CoreResult<StoredCommit> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual = latest_revision_in_transaction(&transaction)?;
        if actual != expected_revision {
            return Err(CoreError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }

        let revision = actual + 1;
        let sequence = latest_sequence_in_transaction(&transaction)? + 1;
        let state_json = serde_json::to_string(&state)?;
        let checksum = checksum(&self.realm, revision, operation, &state_json);
        let committed_at = now();

        transaction.execute(
            "INSERT INTO snapshots (revision, operation, state_json, checksum, committed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![revision, operation, state_json, checksum, committed_at],
        )?;
        transaction.execute(
            "INSERT INTO events (sequence, revision, kind, payload_json, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![sequence, revision, operation, serde_json::to_string(&state)?, committed_at],
        )?;
        trim_history(
            &transaction,
            self.snapshot_retention,
            self.event_retention,
        )?;
        transaction.commit()?;

        Ok(StoredCommit {
            snapshot: StoredSnapshot {
                revision,
                state: state.clone(),
                checksum,
                committed_at: committed_at.clone(),
            },
            event: StoredEvent {
                sequence,
                revision,
                kind: operation.to_owned(),
                occurred_at: committed_at,
                payload: state,
            },
        })
    }

    pub fn append_event(
        &self,
        revision: u64,
        kind: &str,
        payload: Value,
    ) -> CoreResult<StoredEvent> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sequence = latest_sequence_in_transaction(&transaction)? + 1;
        let occurred_at = now();
        transaction.execute(
            "INSERT INTO events (sequence, revision, kind, payload_json, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![sequence, revision, kind, serde_json::to_string(&payload)?, occurred_at],
        )?;
        trim_history(&transaction, self.snapshot_retention, self.event_retention)?;
        transaction.commit()?;

        Ok(StoredEvent {
            sequence,
            revision,
            kind: kind.to_owned(),
            occurred_at,
            payload,
        })
    }

    pub fn verify_latest(&self) -> CoreResult<bool> {
        let Some(snapshot) = self.latest_snapshot()? else {
            return Ok(true);
        };
        let connection = self.lock()?;
        let operation: String = connection.query_row(
            "SELECT operation FROM snapshots WHERE revision = ?1",
            [snapshot.revision],
            |row| row.get(0),
        )?;
        let state_json = serde_json::to_string(&snapshot.state)?;
        Ok(snapshot.checksum == checksum(&self.realm, snapshot.revision, &operation, &state_json))
    }

    fn lock(&self) -> CoreResult<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| CoreError::LockPoisoned)
    }
}

fn load_latest(connection: &Connection) -> CoreResult<Option<StoredSnapshot>> {
    connection
        .query_row(
            "SELECT revision, state_json, checksum, committed_at
             FROM snapshots ORDER BY revision DESC LIMIT 1",
            [],
            |row| {
                let state_json: String = row.get(1)?;
                Ok((
                    row.get::<_, u64>(0)?,
                    state_json,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .map(|(revision, state_json, checksum, committed_at)| {
            Ok(StoredSnapshot {
                revision,
                state: serde_json::from_str(&state_json)?,
                checksum,
                committed_at,
            })
        })
        .transpose()
}

fn latest_revision_in_transaction(transaction: &Transaction<'_>) -> CoreResult<u64> {
    transaction
        .query_row("SELECT COALESCE(MAX(revision), 0) FROM snapshots", [], |row| row.get(0))
        .map_err(Into::into)
}

fn latest_sequence_in_transaction(transaction: &Transaction<'_>) -> CoreResult<u64> {
    transaction
        .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| row.get(0))
        .map_err(Into::into)
}

fn trim_history(
    transaction: &Transaction<'_>,
    snapshot_retention: usize,
    event_retention: usize,
) -> CoreResult<()> {
    transaction.execute(
        "DELETE FROM snapshots WHERE revision NOT IN
         (SELECT revision FROM snapshots ORDER BY revision DESC LIMIT ?1)",
        [snapshot_retention as i64],
    )?;
    transaction.execute(
        "DELETE FROM events WHERE sequence NOT IN
         (SELECT sequence FROM events ORDER BY sequence DESC LIMIT ?1)",
        [event_retention as i64],
    )?;
    Ok(())
}

fn checksum(realm: &RealmId, revision: u64, operation: &str, state_json: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(realm.storage_key());
    digest.update(b"\0");
    digest.update(revision.to_le_bytes());
    digest.update(b"\0");
    digest.update(operation.as_bytes());
    digest.update(b"\0");
    digest.update(state_json.as_bytes());
    format!("{:x}", digest.finalize())
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::SqliteStore;
    use crate::{CoreError, RealmId};

    #[test]
    fn commits_are_revisioned_and_conflicts_are_rejected() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("node.db");
        let store = SqliteStore::open(&path, RealmId::Live, 4, 8).unwrap();

        let first = store.commit_state(0, "state.updated", json!({"value": 1})).unwrap();
        assert_eq!(first.snapshot.revision, 1);
        assert!(matches!(
            store.commit_state(0, "state.updated", json!({"value": 2})),
            Err(CoreError::RevisionConflict { actual: 1, .. })
        ));
        assert!(store.verify_latest().unwrap());
    }

    #[test]
    fn one_database_cannot_cross_realms() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("node.db");
        SqliteStore::open(&path, RealmId::Live, 4, 8).unwrap();

        let result = SqliteStore::open(
            &path,
            RealmId::Lab("scenario-a".to_owned()),
            4,
            8,
        );
        assert!(matches!(result, Err(CoreError::RealmMismatch { .. })));
    }
}
