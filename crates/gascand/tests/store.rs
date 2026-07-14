use camino::Utf8PathBuf;
use gascan_core::sandbox::SandboxId;
use gascand::{
    ActualState, DesiredState, ImageResolution, OperationKind, OperationStatus, SandboxRecord,
    SetupResolution, Store, StoreError, ToolResolution,
};
use serde_json::json;
use std::error::Error;
use std::process::Command;

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
        Err(StoreError::DuplicateSandboxId { .. })
    ));

    let mut duplicate_root = fixture("/workspace/one");
    duplicate_root.id = SandboxId::from_root("other", &duplicate_root.canonical_root);
    assert!(matches!(
        store.put_sandbox(&duplicate_root),
        Err(StoreError::DuplicateCanonicalRoot { .. })
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
fn failed_create_can_record_successful_cleanup_to_absent() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let store = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    let operation = store.begin_operation(&sandbox, OperationKind::Create)?;
    let failed = store.fail_operation(
        operation.id,
        ActualState::Absent,
        "create_failed",
        json!({}),
    )?;
    drop(store);

    let reopened = Store::open(path)?;
    assert_eq!(failed.status, OperationStatus::Failed);
    assert!(reopened.pending_operations()?.is_empty());
    assert_eq!(
        reopened
            .sandbox(&sandbox.id)?
            .map(|record| record.actual_state),
        Some(ActualState::Absent)
    );
    Ok(())
}

#[test]
fn failed_destroy_can_restore_the_verified_running_or_stopped_state() -> TestResult {
    for restored in [ActualState::Running, ActualState::Stopped] {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("state.db"))?;
        let mut sandbox = fixture("/workspace/one");
        store.put_sandbox(&sandbox)?;
        sandbox.actual_state = ActualState::Running;
        store.put_sandbox(&sandbox)?;
        sandbox.actual_state = ActualState::Destroying;
        let operation = store.begin_operation(&sandbox, OperationKind::Destroy)?;
        store.fail_operation(operation.id, restored, "destroy_failed", json!({}))?;
        assert_eq!(
            store
                .sandbox(&sandbox.id)?
                .map(|record| record.actual_state),
            Some(restored)
        );
    }
    Ok(())
}

#[test]
fn completed_operations_do_not_use_failure_rollback_edges() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let sandbox = fixture("/workspace/one");
    let create = store.begin_operation(&sandbox, OperationKind::Create)?;
    assert!(matches!(
        store.complete_operation(create.id, ActualState::Absent),
        Err(StoreError::InvalidTransition { .. })
    ));
    Ok(())
}

#[test]
fn one_pending_operation_per_sandbox_is_durable() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let store = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    let first = store.begin_operation(&sandbox, OperationKind::Create)?;
    drop(store);
    let reopened = Store::open(path)?;
    assert!(matches!(
        reopened.begin_operation(&sandbox, OperationKind::Apply),
        Err(StoreError::PendingOperationExists { sandbox_id }) if sandbox_id == sandbox.id
    ));
    assert_eq!(reopened.pending_operations()?, vec![first]);
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
fn malformed_version_one_schemas_are_rejected_at_open() -> TestResult {
    const MIGRATION: &str = include_str!("../migrations/001_initial.sql");
    for (name, statements) in [
        ("missing-table", "DROP TABLE operation_events;"),
        (
            "missing-column",
            "ALTER TABLE sandboxes DROP COLUMN tool_resolution_details;",
        ),
        (
            "missing-trigger",
            "DROP TRIGGER operation_events_no_update;",
        ),
        (
            "missing-pending-index",
            "DROP INDEX one_pending_operation_per_sandbox;",
        ),
        ("missing-foreign-key", ""),
        ("missing-event-foreign-key", ""),
        ("missing-root-unique", ""),
        ("nullable-required-column", ""),
        ("missing-version-check", ""),
    ] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join(format!("{name}.db"));
        let connection = rusqlite::Connection::open(&path)?;
        if statements.is_empty() {
            let malformed = match name {
                "missing-foreign-key" => MIGRATION.replace(
                    "sandbox_id TEXT NOT NULL REFERENCES sandboxes(id)",
                    "sandbox_id TEXT NOT NULL",
                ),
                "missing-event-foreign-key" => MIGRATION.replace(
                    "operation_id INTEGER NOT NULL REFERENCES operations(id)",
                    "operation_id INTEGER NOT NULL",
                ),
                "missing-root-unique" => MIGRATION.replace(
                    "canonical_root TEXT NOT NULL UNIQUE",
                    "canonical_root TEXT NOT NULL",
                ),
                "nullable-required-column" => {
                    MIGRATION.replace("desired_state TEXT NOT NULL", "desired_state TEXT")
                }
                "missing-version-check" => {
                    MIGRATION.replace("PRIMARY KEY CHECK (singleton = 1)", "PRIMARY KEY")
                }
                other => return Err(format!("unhandled malformed schema {other}").into()),
            };
            connection.execute_batch(&malformed)?;
        } else {
            connection.execute_batch(MIGRATION)?;
            connection.execute_batch(statements)?;
        }
        drop(connection);
        assert!(matches!(
            Store::open(path),
            Err(StoreError::SchemaMismatch(_))
        ));
    }

    let temp = tempfile::tempdir()?;
    let path = temp.path().join("multiple-versions.db");
    let connection = rusqlite::Connection::open(&path)?;
    connection.execute_batch(MIGRATION)?;
    connection.execute_batch(
        "DROP TABLE schema_version;
         CREATE TABLE schema_version (singleton INTEGER, version INTEGER NOT NULL);
         INSERT INTO schema_version VALUES (1, 1), (2, 1);",
    )?;
    drop(connection);
    assert!(matches!(
        Store::open(path),
        Err(StoreError::SchemaMismatch(_))
    ));
    Ok(())
}

#[test]
fn partial_version_one_schema_is_rejected_at_open() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("partial.db");
    let connection = rusqlite::Connection::open(&path)?;
    connection.execute_batch(
        "CREATE TABLE schema_version (singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL);
         INSERT INTO schema_version VALUES (1, 1);",
    )?;
    drop(connection);
    assert!(matches!(
        Store::open(path),
        Err(StoreError::SchemaMismatch(_))
    ));
    Ok(())
}

fn run_crash_child(mode: &str, path: &std::path::Path) -> TestResult {
    let status = Command::new(std::env::current_exe()?)
        .args(["--exact", "sqlite_crash_child", "--nocapture"])
        .env("GASCAN_STORE_CRASH_MODE", mode)
        .env("GASCAN_STORE_CRASH_DB", path)
        .status()?;
    assert!(!status.success());
    Ok(())
}

#[test]
fn subprocess_crash_rolls_back_partial_begin_transaction() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("begin-crash.db");
    drop(Store::open(&path)?);
    run_crash_child("begin", &path)?;
    let reopened = Store::open(path)?;
    assert!(reopened.list_sandboxes()?.is_empty());
    assert!(reopened.pending_operations()?.is_empty());
    Ok(())
}

#[test]
fn subprocess_crash_rolls_back_partial_terminal_transaction() -> TestResult {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("terminal-crash.db");
    let store = Store::open(&path)?;
    let sandbox = fixture("/workspace/one");
    let pending = store.begin_operation(&sandbox, OperationKind::Create)?;
    drop(store);
    run_crash_child("terminal", &path)?;
    let reopened = Store::open(path)?;
    assert_eq!(reopened.pending_operations()?, vec![pending]);
    assert_eq!(
        reopened
            .sandbox(&sandbox.id)?
            .map(|record| record.actual_state),
        Some(ActualState::Creating)
    );
    Ok(())
}

#[test]
fn sqlite_crash_child() -> TestResult {
    let Ok(mode) = std::env::var("GASCAN_STORE_CRASH_MODE") else {
        return Ok(());
    };
    let path = std::env::var("GASCAN_STORE_CRASH_DB")?;
    let connection = rusqlite::Connection::open(path)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; BEGIN IMMEDIATE;")?;
    match mode.as_str() {
        "begin" => {
            let sandbox = fixture("/workspace/crashed");
            connection.execute(
                "INSERT INTO sandboxes (id, canonical_root, desired_state, actual_state) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![sandbox.id.as_str(), sandbox.canonical_root.as_str(), "running", "creating"],
            )?;
            connection.execute(
                "INSERT INTO operations (sandbox_id, kind, status) VALUES (?1, ?2, ?3)",
                rusqlite::params![sandbox.id.as_str(), "create", "pending"],
            )?;
        }
        "terminal" => {
            connection.execute("UPDATE sandboxes SET actual_state = ?1", ["running"])?;
            connection.execute("UPDATE operations SET status = ?1", ["completed"])?;
            connection.execute(
                "INSERT INTO operation_events (operation_id, status) SELECT id, ?1 FROM operations",
                ["completed"],
            )?;
        }
        other => return Err(format!("unknown crash mode {other}").into()),
    }
    std::process::abort();
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
