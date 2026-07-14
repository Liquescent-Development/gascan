use camino::Utf8PathBuf;
use gascan_core::sandbox::SandboxId;
use gascand::{
    ActualState, DesiredState, ImageResolution, OperationKind, OperationStatus, SandboxRecord,
    SetupResolution, Store, StoreError, ToolResolution,
};
use serde_json::json;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn fixture(root: &str) -> SandboxRecord {
    let canonical_root = Utf8PathBuf::from(root);
    SandboxRecord {
        id: SandboxId::from_root("fixture", &canonical_root),
        canonical_root,
        desired_state: DesiredState::Running,
        actual_state: ActualState::Creating,
        setup_resolution: Some(SetupResolution::new(
            1,
            json!({"path":"setup.sh","digest":"abc"}),
        )),
        tool_resolution: Some(ToolResolution::new(1, json!({"node":"22.1.0"}))),
        image_resolution: Some(ImageResolution::new(1, json!({"digest":"sha256:abc"}))),
    }
}

#[test]
fn pending_operation_survives_reopen() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let store = Store::open(&path)?;
    let operation = store.begin_operation(&fixture("/workspace/one"), OperationKind::Create)?;
    drop(store);

    let reopened = Store::open(&path)?;
    assert_eq!(reopened.pending_operations()?, vec![operation]);
    Ok(())
}

#[test]
fn sandbox_round_trips_and_lists_in_id_order() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let one = fixture("/workspace/one");
    let mut two = fixture("/workspace/two");
    two.desired_state = DesiredState::Stopped;
    store.put_sandbox(&two)?;
    store.put_sandbox(&one)?;

    assert_eq!(store.sandbox(&one.id)?, Some(one.clone()));
    let mut expected = vec![one, two];
    expected.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    assert_eq!(store.list_sandboxes()?, expected);
    Ok(())
}

#[test]
fn sandbox_id_and_canonical_root_are_both_unique() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let one = fixture("/workspace/one");
    store.put_sandbox(&one)?;

    let mut duplicate_id = fixture("/workspace/two");
    duplicate_id.id = one.id.clone();
    assert!(matches!(
        store.put_sandbox(&duplicate_id),
        Err(StoreError::Conflict(_))
    ));

    let mut duplicate_root = fixture("/workspace/one");
    duplicate_root.id = SandboxId::from_root("other", &duplicate_root.canonical_root);
    assert!(matches!(
        store.put_sandbox(&duplicate_root),
        Err(StoreError::Conflict(_))
    ));
    Ok(())
}

#[test]
fn lifecycle_transitions_are_validated_before_commit() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let mut sandbox = fixture("/workspace/one");
    store.put_sandbox(&sandbox)?;
    sandbox.actual_state = ActualState::Running;
    store.put_sandbox(&sandbox)?;
    sandbox.actual_state = ActualState::Creating;
    assert!(matches!(
        store.put_sandbox(&sandbox),
        Err(StoreError::InvalidTransition { .. })
    ));
    assert_eq!(
        store
            .sandbox(&sandbox.id)?
            .map(|record| record.actual_state),
        Some(ActualState::Running)
    );
    Ok(())
}

#[test]
fn operations_complete_or_fail_once_and_events_are_append_only() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let store = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    let completed = store.begin_operation(&sandbox, OperationKind::Create)?;
    let completed = store.complete_operation(completed.id, ActualState::Running)?;
    assert_eq!(completed.status, OperationStatus::Completed);
    assert!(matches!(
        store.complete_operation(completed.id, ActualState::Running),
        Err(StoreError::InvalidTransition { .. })
    ));

    let mut stopped = sandbox.clone();
    stopped.actual_state = ActualState::Stopped;
    let failed = store.begin_operation(&stopped, OperationKind::Start)?;
    let failed = store.fail_operation(
        failed.id,
        ActualState::Stopped,
        "runtime_error",
        json!({"retryable":true}),
    )?;
    assert_eq!(failed.status, OperationStatus::Failed);
    assert_eq!(store.pending_operations()?, Vec::new());
    assert_eq!(
        store
            .operation_events(completed.id)?
            .iter()
            .map(|event| event.status)
            .collect::<Vec<_>>(),
        vec![OperationStatus::Pending, OperationStatus::Completed]
    );
    let connection = rusqlite::Connection::open(path)?;
    assert!(
        connection
            .execute(
                "UPDATE operation_events SET status = ?1 WHERE operation_id = ?2",
                rusqlite::params!["failed", completed.id],
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn invalid_terminal_transition_rolls_back_operation_and_sandbox_atomically() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let store = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    let operation = store.begin_operation(&sandbox, OperationKind::Create)?;
    assert!(matches!(
        store.fail_operation(operation.id, ActualState::Absent, "crash", json!({})),
        Err(StoreError::InvalidTransition { .. })
    ));
    drop(store);

    let reopened = Store::open(path)?;
    assert_eq!(reopened.pending_operations()?, vec![operation]);
    assert_eq!(
        reopened
            .sandbox(&sandbox.id)?
            .map(|record| record.actual_state),
        Some(ActualState::Creating)
    );
    Ok(())
}

#[test]
fn separate_connections_can_read_while_the_store_is_open() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let writer = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    writer.put_sandbox(&sandbox)?;
    let reader = Store::open(&path)?;
    assert_eq!(reader.sandbox(&sandbox.id)?, Some(sandbox));
    Ok(())
}

#[test]
fn wal_reader_keeps_reading_during_an_uncommitted_write() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let store = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    store.put_sandbox(&sandbox)?;

    let writer = rusqlite::Connection::open(&path)?;
    assert_eq!(
        writer.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
        "wal"
    );
    writer.execute_batch("BEGIN IMMEDIATE")?;
    writer.execute(
        "UPDATE sandboxes SET desired_state = ?1 WHERE id = ?2",
        rusqlite::params!["stopped", sandbox.id.as_str()],
    )?;

    assert_eq!(store.sandbox(&sandbox.id)?, Some(sandbox));
    writer.execute_batch("ROLLBACK")?;
    Ok(())
}

#[test]
fn newer_and_unknown_schema_versions_are_rejected() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    drop(Store::open(&path)?);
    let connection = rusqlite::Connection::open(&path)?;
    connection.execute("UPDATE schema_version SET version = ?1", [2])?;
    drop(connection);
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::UnsupportedSchemaVersion(2))
    ));

    let empty = temp.path().join("unknown.db");
    let connection = rusqlite::Connection::open(&empty)?;
    connection.execute("PRAGMA user_version = 9", [])?;
    drop(connection);
    assert!(matches!(Store::open(empty), Err(StoreError::UnknownSchema)));
    Ok(())
}

#[test]
fn versioned_resolution_records_round_trip() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let sandbox = fixture("/workspace/one");
    store.put_sandbox(&sandbox)?;
    let loaded = store.sandbox(&sandbox.id)?.ok_or("sandbox missing")?;
    assert_eq!(loaded.setup_resolution, sandbox.setup_resolution);
    assert_eq!(loaded.tool_resolution, sandbox.tool_resolution);
    assert_eq!(loaded.image_resolution, sandbox.image_resolution);
    Ok(())
}
