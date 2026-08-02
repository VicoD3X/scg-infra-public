use scg_core::{CommitRequest, CoreError, RealmId, Runtime, RuntimeConfig, RuntimePhase};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn clean_restart_preserves_revision_and_state() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("live.db");

    {
        let runtime = Runtime::open(RuntimeConfig::new(RealmId::Live, &database)).unwrap();
        runtime.start().unwrap();
        runtime
            .commit(CommitRequest {
                expected_revision: 0,
                operation: "capacity.updated".to_owned(),
                state: json!({"workerLimit": 8, "queueLimit": 256}),
            })
            .unwrap();
        runtime.stop().unwrap();
    }

    let reopened = Runtime::open(RuntimeConfig::new(RealmId::Live, &database)).unwrap();
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(snapshot.phase, RuntimePhase::Stopped);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.state["workerLimit"], 8);
}

#[test]
fn realm_identity_is_checked_before_runtime_start() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("realm.db");
    Runtime::open(RuntimeConfig::new(RealmId::Live, &database)).unwrap();

    let result = Runtime::open(RuntimeConfig::new(
        RealmId::Lab("fault-suite".to_owned()),
        &database,
    ));
    assert!(matches!(result, Err(CoreError::RealmMismatch { .. })));
}
