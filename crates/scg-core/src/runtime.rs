use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::{
    CONTRACT_VERSION, CommitReceipt, CommitRequest, CoreError, CoreResult, EventEnvelope,
    HealthSnapshot, RealmId, RuntimePhase, RuntimeSnapshot, ServiceGraph, ServiceSnapshot,
    ServiceStatus,
    store::{SqliteStore, StoredEvent},
};

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub realm: RealmId,
    pub database_path: PathBuf,
    pub event_retention: usize,
    pub snapshot_retention: usize,
}

impl RuntimeConfig {
    pub fn new(realm: RealmId, database_path: impl Into<PathBuf>) -> Self {
        Self {
            realm,
            database_path: database_path.into(),
            event_retention: 1000,
            snapshot_retention: 20,
        }
    }
}

#[derive(Debug)]
struct RuntimeState {
    phase: RuntimePhase,
    revision: u64,
    clean_shutdown: bool,
    services: Vec<ServiceSnapshot>,
    last_commit_at: Option<String>,
    state: Value,
}

pub struct Runtime {
    realm: RealmId,
    graph: ServiceGraph,
    store: Arc<SqliteStore>,
    state: Mutex<RuntimeState>,
    events: broadcast::Sender<EventEnvelope>,
}

impl Runtime {
    pub fn open(config: RuntimeConfig) -> CoreResult<Self> {
        let graph = ServiceGraph::control_plane();
        let store = Arc::new(SqliteStore::open(
            &config.database_path,
            config.realm.clone(),
            config.snapshot_retention,
            config.event_retention,
        )?);
        if !store.verify_latest()? {
            return Err(CoreError::ChecksumMismatch);
        }

        let latest = store.latest_snapshot()?;
        let checkpoint = store.checkpoint()?;
        let clean_shutdown = checkpoint.map(|value| value.clean_shutdown).unwrap_or(true);
        let phase = if clean_shutdown {
            RuntimePhase::Stopped
        } else {
            RuntimePhase::Degraded
        };
        let services = graph
            .start_order()
            .iter()
            .enumerate()
            .map(|(index, service)| ServiceSnapshot {
                id: service.id.clone(),
                status: ServiceStatus::Stopped,
                start_order: index,
            })
            .collect();
        let (events, _) = broadcast::channel(256);

        Ok(Self {
            realm: config.realm,
            graph,
            store,
            state: Mutex::new(RuntimeState {
                phase,
                revision: latest.as_ref().map(|value| value.revision).unwrap_or(0),
                clean_shutdown,
                services,
                last_commit_at: latest.as_ref().map(|value| value.committed_at.clone()),
                state: latest.map(|value| value.state).unwrap_or_else(|| json!({})),
            }),
            events,
        })
    }

    pub fn start(&self) -> CoreResult<RuntimeSnapshot> {
        let mut state = self.lock()?;
        if state.phase == RuntimePhase::Ready {
            return Ok(self.snapshot_from(&state));
        }
        if !matches!(state.phase, RuntimePhase::Stopped | RuntimePhase::Degraded) {
            return Err(CoreError::InvalidTransition {
                from: state.phase.to_string(),
                to: RuntimePhase::Ready.to_string(),
            });
        }
        if !self.store.verify_latest()? {
            state.phase = RuntimePhase::Failed;
            return Err(CoreError::ChecksumMismatch);
        }

        state.phase = RuntimePhase::Starting;
        state.clean_shutdown = false;
        self.store
            .mark_checkpoint(RuntimePhase::Starting, state.revision, false)?;

        for service in &mut state.services {
            service.status = ServiceStatus::Starting;
            service.status = ServiceStatus::Ready;
        }
        state.phase = RuntimePhase::Ready;
        self.store
            .mark_checkpoint(RuntimePhase::Ready, state.revision, false)?;
        let event = self.store.append_event(
            state.revision,
            "runtime.ready",
            json!({"services": state.services.len()}),
        )?;
        self.publish(event);

        Ok(self.snapshot_from(&state))
    }

    pub fn stop(&self) -> CoreResult<RuntimeSnapshot> {
        let mut state = self.lock()?;
        if state.phase == RuntimePhase::Stopped {
            return Ok(self.snapshot_from(&state));
        }
        if !matches!(
            state.phase,
            RuntimePhase::Ready | RuntimePhase::Degraded | RuntimePhase::Failed
        ) {
            return Err(CoreError::InvalidTransition {
                from: state.phase.to_string(),
                to: RuntimePhase::Stopped.to_string(),
            });
        }

        state.phase = RuntimePhase::Stopping;
        for service in self.graph.stop_order() {
            if let Some(snapshot) = state
                .services
                .iter_mut()
                .find(|value| value.id == service.id)
            {
                snapshot.status = ServiceStatus::Stopped;
            }
        }
        state.phase = RuntimePhase::Stopped;
        state.clean_shutdown = true;
        self.store
            .mark_checkpoint(RuntimePhase::Stopped, state.revision, true)?;
        let event =
            self.store
                .append_event(state.revision, "runtime.stopped", json!({"clean": true}))?;
        self.publish(event);

        Ok(self.snapshot_from(&state))
    }

    pub fn commit(&self, request: CommitRequest) -> CoreResult<CommitReceipt> {
        validate_operation(&request.operation)?;
        let mut state = self.lock()?;
        if state.phase != RuntimePhase::Ready {
            return Err(CoreError::InvalidTransition {
                from: state.phase.to_string(),
                to: "Commit".to_owned(),
            });
        }

        let committed = self.store.commit_state(
            request.expected_revision,
            &request.operation,
            request.state,
        )?;
        state.revision = committed.snapshot.revision;
        state.last_commit_at = Some(committed.snapshot.committed_at.clone());
        state.state = committed.snapshot.state.clone();
        self.store
            .mark_checkpoint(RuntimePhase::Ready, state.revision, false)?;
        self.publish(committed.event.clone());

        Ok(CommitReceipt {
            revision: committed.snapshot.revision,
            event_sequence: committed.event.sequence,
            checksum: committed.snapshot.checksum,
            committed_at: committed.snapshot.committed_at,
        })
    }

    pub fn snapshot(&self) -> CoreResult<RuntimeSnapshot> {
        let state = self.lock()?;
        Ok(self.snapshot_from(&state))
    }

    pub fn health(&self) -> CoreResult<HealthSnapshot> {
        let state = self.lock()?;
        let storage_verified = self.store.verify_latest()?;
        let services_ready = state
            .services
            .iter()
            .all(|service| service.status == ServiceStatus::Ready);
        Ok(HealthSnapshot {
            contract_version: CONTRACT_VERSION,
            ready: state.phase == RuntimePhase::Ready && services_ready && storage_verified,
            phase: state.phase,
            revision: state.revision,
            realm: self.realm.clone(),
            storage_verified,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    fn lock(&self) -> CoreResult<MutexGuard<'_, RuntimeState>> {
        self.state.lock().map_err(|_| CoreError::LockPoisoned)
    }

    fn snapshot_from(&self, state: &RuntimeState) -> RuntimeSnapshot {
        RuntimeSnapshot {
            contract_version: CONTRACT_VERSION,
            realm: self.realm.clone(),
            phase: state.phase,
            revision: state.revision,
            clean_shutdown: state.clean_shutdown,
            services: state.services.clone(),
            last_commit_at: state.last_commit_at.clone(),
            state: state.state.clone(),
        }
    }

    fn publish(&self, event: StoredEvent) {
        let _ = self.events.send(EventEnvelope {
            contract_version: CONTRACT_VERSION,
            sequence: event.sequence,
            revision: event.revision,
            kind: event.kind,
            occurred_at: event.occurred_at,
            payload: event.payload,
        });
    }
}

fn validate_operation(operation: &str) -> CoreResult<()> {
    let valid = !operation.is_empty()
        && operation.len() <= 64
        && operation.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '-' | '_')
        });
    if valid {
        Ok(())
    } else {
        Err(CoreError::InvalidOperation(
            "operation names must use 1-64 lowercase letters, digits, dots, hyphens, or underscores"
                .to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::{Runtime, RuntimeConfig};
    use crate::{CommitRequest, CoreError, RealmId, RuntimePhase};

    #[test]
    fn lifecycle_and_revision_checks_are_enforced() {
        let directory = tempdir().unwrap();
        let runtime = Runtime::open(RuntimeConfig::new(
            RealmId::Live,
            directory.path().join("node.db"),
        ))
        .unwrap();

        assert_eq!(runtime.snapshot().unwrap().phase, RuntimePhase::Stopped);
        assert_eq!(runtime.start().unwrap().phase, RuntimePhase::Ready);

        let receipt = runtime
            .commit(CommitRequest {
                expected_revision: 0,
                operation: "node.configured".to_owned(),
                state: json!({"workers": 4}),
            })
            .unwrap();
        assert_eq!(receipt.revision, 1);
        assert!(matches!(
            runtime.commit(CommitRequest {
                expected_revision: 0,
                operation: "node.configured".to_owned(),
                state: json!({"workers": 8}),
            }),
            Err(CoreError::RevisionConflict { actual: 1, .. })
        ));

        let stopped = runtime.stop().unwrap();
        assert!(stopped.clean_shutdown);
        assert_eq!(stopped.phase, RuntimePhase::Stopped);
    }

    #[test]
    fn unclean_checkpoint_reopens_as_degraded() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("node.db");
        {
            let runtime = Runtime::open(RuntimeConfig::new(RealmId::Live, &path)).unwrap();
            runtime.start().unwrap();
        }

        let reopened = Runtime::open(RuntimeConfig::new(RealmId::Live, &path)).unwrap();
        assert_eq!(reopened.snapshot().unwrap().phase, RuntimePhase::Degraded);
        assert_eq!(reopened.start().unwrap().phase, RuntimePhase::Ready);
    }
}
