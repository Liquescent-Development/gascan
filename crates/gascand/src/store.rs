use camino::Utf8PathBuf;
use gascan_core::sandbox::{SandboxId, SandboxIdError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use thiserror::Error;

const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_MIGRATION: &str = include_str!("../migrations/001_initial.sql");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesiredState {
    Absent,
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActualState {
    Absent,
    Creating,
    Running,
    Stopped,
    Destroying,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    Create,
    Apply,
    Start,
    Stop,
    Destroy,
    Reconcile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationStatus {
    Pending,
    Completed,
    Failed,
}

macro_rules! resolution_record {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            pub version: u32,
            pub details: Value,
        }

        impl $name {
            pub const fn new(version: u32, details: Value) -> Self {
                Self { version, details }
            }
        }
    };
}

resolution_record!(SetupResolution);
resolution_record!(ToolResolution);
resolution_record!(ImageResolution);

#[derive(Clone, Debug, PartialEq)]
pub struct SandboxRecord {
    pub id: SandboxId,
    pub canonical_root: Utf8PathBuf,
    pub desired_state: DesiredState,
    pub actual_state: ActualState,
    pub setup_resolution: Option<SetupResolution>,
    pub tool_resolution: Option<ToolResolution>,
    pub image_resolution: Option<ImageResolution>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperationRecord {
    pub id: i64,
    pub sandbox_id: SandboxId,
    pub kind: OperationKind,
    pub status: OperationStatus,
    pub error_code: Option<String>,
    pub error_details: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperationEvent {
    pub sequence: i64,
    pub operation_id: i64,
    pub status: OperationStatus,
    pub details: Option<Value>,
}

pub struct Store {
    connection: Mutex<Connection>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("stored sandbox ID is invalid: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error("database lock was poisoned")]
    LockPoisoned,
    #[error("database has no recognized schema")]
    UnknownSchema,
    #[error("unsupported database schema version {0}")]
    UnsupportedSchemaVersion(i64),
    #[error("sandbox ID {sandbox_id} already belongs to canonical root {existing_root}")]
    DuplicateSandboxId {
        sandbox_id: SandboxId,
        existing_root: Utf8PathBuf,
    },
    #[error("canonical root {canonical_root} already belongs to sandbox ID {existing_id}")]
    DuplicateCanonicalRoot {
        canonical_root: Utf8PathBuf,
        existing_id: SandboxId,
    },
    #[error("sandbox {sandbox_id} already has a pending operation")]
    PendingOperationExists { sandbox_id: SandboxId },
    #[error("database schema does not match version 1: {0}")]
    SchemaMismatch(String),
    #[error("invalid transition from {from} to {to}")]
    InvalidTransition { from: String, to: String },
    #[error("operation {0} does not exist")]
    OperationNotFound(i64),
    #[error("stored value is invalid: {0}")]
    CorruptData(String),
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        initialize_schema(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn put_sandbox(&self, sandbox: &SandboxRecord) -> Result<(), StoreError> {
        validate_resolutions(sandbox)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        put_sandbox_in(&transaction, sandbox)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn sandbox(&self, id: &SandboxId) -> Result<Option<SandboxRecord>, StoreError> {
        let connection = self.lock()?;
        load_sandbox(&connection, id.as_str())
    }

    pub fn list_sandboxes(&self) -> Result<Vec<SandboxRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&format!("{SANDBOX_SELECT} ORDER BY id"))?;
        let rows = statement.query_map([], sandbox_from_row)?;
        collect_rows(rows)
    }

    pub fn begin_operation(
        &self,
        sandbox: &SandboxRecord,
        kind: OperationKind,
    ) -> Result<OperationRecord, StoreError> {
        validate_resolutions(sandbox)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        put_sandbox_in(&transaction, sandbox)?;
        let has_pending: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE sandbox_id = ?1 AND status = ?2)",
            params![sandbox.id.as_str(), OperationStatus::Pending.as_db()],
            |row| row.get(0),
        )?;
        if has_pending {
            return Err(StoreError::PendingOperationExists {
                sandbox_id: sandbox.id.clone(),
            });
        }
        transaction.execute(
            "INSERT INTO operations (sandbox_id, kind, status) VALUES (?1, ?2, ?3)",
            params![
                sandbox.id.as_str(),
                kind.as_db(),
                OperationStatus::Pending.as_db()
            ],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO operation_events (operation_id, status) VALUES (?1, ?2)",
            params![id, OperationStatus::Pending.as_db()],
        )?;
        let operation =
            load_operation(&transaction, id)?.ok_or(StoreError::OperationNotFound(id))?;
        transaction.commit()?;
        Ok(operation)
    }

    pub fn complete_operation(
        &self,
        id: i64,
        actual_state: ActualState,
    ) -> Result<OperationRecord, StoreError> {
        self.finish_operation(id, actual_state, OperationStatus::Completed, None, None)
    }

    pub fn fail_operation(
        &self,
        id: i64,
        actual_state: ActualState,
        error_code: impl Into<String>,
        error_details: Value,
    ) -> Result<OperationRecord, StoreError> {
        self.finish_operation(
            id,
            actual_state,
            OperationStatus::Failed,
            Some(error_code.into()),
            Some(error_details),
        )
    }

    pub fn pending_operations(&self) -> Result<Vec<OperationRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement =
            connection.prepare(&format!("{OPERATION_SELECT} WHERE status = ?1 ORDER BY id"))?;
        let rows = statement.query_map([OperationStatus::Pending.as_db()], operation_from_row)?;
        collect_rows(rows)
    }

    pub fn operation_events(&self, operation_id: i64) -> Result<Vec<OperationEvent>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sequence, operation_id, status, details FROM operation_events WHERE operation_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map([operation_id], event_from_row)?;
        collect_rows(rows)
    }

    fn finish_operation(
        &self,
        id: i64,
        actual_state: ActualState,
        status: OperationStatus,
        error_code: Option<String>,
        error_details: Option<Value>,
    ) -> Result<OperationRecord, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation =
            load_operation(&transaction, id)?.ok_or(StoreError::OperationNotFound(id))?;
        validate_operation_transition(operation.status, status)?;
        let current_state: String = transaction.query_row(
            "SELECT actual_state FROM sandboxes WHERE id = ?1",
            [operation.sandbox_id.as_str()],
            |row| row.get(0),
        )?;
        let current_state = ActualState::from_db(&current_state)?;
        validate_terminal_actual_transition(current_state, actual_state, operation.kind, status)?;
        let details_json = error_details
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        transaction.execute(
            "UPDATE sandboxes SET actual_state = ?1 WHERE id = ?2",
            params![actual_state.as_db(), operation.sandbox_id.as_str()],
        )?;
        transaction.execute(
            "UPDATE operations SET status = ?1, error_code = ?2, error_details = ?3 WHERE id = ?4",
            params![status.as_db(), error_code, details_json, id],
        )?;
        transaction.execute(
            "INSERT INTO operation_events (operation_id, status, details) VALUES (?1, ?2, ?3)",
            params![id, status.as_db(), details_json],
        )?;
        let updated = load_operation(&transaction, id)?.ok_or(StoreError::OperationNotFound(id))?;
        transaction.commit()?;
        Ok(updated)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.connection.lock().map_err(|_| StoreError::LockPoisoned)
    }
}

fn initialize_schema(connection: &mut Connection) -> Result<(), StoreError> {
    let has_schema: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version')",
        [],
        |row| row.get(0),
    )?;
    if has_schema {
        validate_table_columns(
            connection,
            "schema_version",
            &[("singleton", "INTEGER", 1, 1), ("version", "INTEGER", 1, 0)],
        )?;
        validate_schema_version_constraint(connection)?;
        let rows: i64 = schema_query(connection, "SELECT COUNT(*) FROM schema_version")?;
        if rows != 1 {
            return Err(StoreError::SchemaMismatch(
                "schema_version must contain exactly one row".to_owned(),
            ));
        }
        let (singleton, version): (i64, i64) = connection
            .query_row("SELECT singleton, version FROM schema_version", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(schema_error)?;
        if singleton != 1 {
            return Err(StoreError::SchemaMismatch(
                "schema_version singleton must equal 1".to_owned(),
            ));
        }
        return if version == SCHEMA_VERSION {
            validate_v1_schema(connection)
        } else {
            Err(StoreError::UnsupportedSchemaVersion(version))
        };
    }
    let object_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    let user_version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if object_count != 0 || user_version != 0 {
        return Err(StoreError::UnknownSchema);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(INITIAL_MIGRATION)?;
    transaction.commit()?;
    Ok(())
}

const SANDBOX_SELECT: &str = "SELECT id, canonical_root, desired_state, actual_state, setup_resolution_version, setup_resolution_details, tool_resolution_version, tool_resolution_details, image_resolution_version, image_resolution_details FROM sandboxes";
const OPERATION_SELECT: &str =
    "SELECT id, sandbox_id, kind, status, error_code, error_details FROM operations";

fn put_sandbox_in(
    transaction: &Transaction<'_>,
    sandbox: &SandboxRecord,
) -> Result<(), StoreError> {
    let existing = load_sandbox(transaction, sandbox.id.as_str())?;
    if let Some(existing) = existing {
        if existing.canonical_root != sandbox.canonical_root {
            return Err(StoreError::DuplicateSandboxId {
                sandbox_id: sandbox.id.clone(),
                existing_root: existing.canonical_root,
            });
        }
        validate_actual_transition(existing.actual_state, sandbox.actual_state)?;
    }
    let root_owner: Option<String> = transaction
        .query_row(
            "SELECT id FROM sandboxes WHERE canonical_root = ?1",
            [sandbox.canonical_root.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(root_owner) = root_owner {
        let existing_id = SandboxId::try_from(root_owner)?;
        if existing_id != sandbox.id {
            return Err(StoreError::DuplicateCanonicalRoot {
                canonical_root: sandbox.canonical_root.clone(),
                existing_id,
            });
        }
    }
    let setup = encode_resolution(sandbox.setup_resolution.as_ref())?;
    let tools = encode_resolution(sandbox.tool_resolution.as_ref())?;
    let image = encode_resolution(sandbox.image_resolution.as_ref())?;
    transaction.execute(
        "INSERT INTO sandboxes (id, canonical_root, desired_state, actual_state, setup_resolution_version, setup_resolution_details, tool_resolution_version, tool_resolution_details, image_resolution_version, image_resolution_details) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(id) DO UPDATE SET desired_state = excluded.desired_state, actual_state = excluded.actual_state, setup_resolution_version = excluded.setup_resolution_version, setup_resolution_details = excluded.setup_resolution_details, tool_resolution_version = excluded.tool_resolution_version, tool_resolution_details = excluded.tool_resolution_details, image_resolution_version = excluded.image_resolution_version, image_resolution_details = excluded.image_resolution_details",
        params![sandbox.id.as_str(), sandbox.canonical_root.as_str(), sandbox.desired_state.as_db(), sandbox.actual_state.as_db(), setup.0, setup.1, tools.0, tools.1, image.0, image.1],
    )?;
    Ok(())
}

fn load_sandbox(connection: &Connection, id: &str) -> Result<Option<SandboxRecord>, StoreError> {
    let mut statement = connection.prepare(&format!("{SANDBOX_SELECT} WHERE id = ?1"))?;
    let raw = statement.query_row([id], raw_sandbox_from_row).optional()?;
    raw.map(SandboxRecord::try_from).transpose()
}

fn load_operation(connection: &Connection, id: i64) -> Result<Option<OperationRecord>, StoreError> {
    let mut statement = connection.prepare(&format!("{OPERATION_SELECT} WHERE id = ?1"))?;
    let raw = statement
        .query_row([id], raw_operation_from_row)
        .optional()?;
    raw.map(OperationRecord::try_from).transpose()
}

struct RawSandbox {
    id: String,
    root: String,
    desired: String,
    actual: String,
    setup: (Option<u32>, Option<String>),
    tools: (Option<u32>, Option<String>),
    image: (Option<u32>, Option<String>),
}

impl TryFrom<RawSandbox> for SandboxRecord {
    type Error = StoreError;

    fn try_from(raw: RawSandbox) -> Result<Self, Self::Error> {
        Ok(Self {
            id: SandboxId::try_from(raw.id)?,
            canonical_root: Utf8PathBuf::from(raw.root),
            desired_state: DesiredState::from_db(&raw.desired)?,
            actual_state: ActualState::from_db(&raw.actual)?,
            setup_resolution: decode_resolution(raw.setup, SetupResolution::new)?,
            tool_resolution: decode_resolution(raw.tools, ToolResolution::new)?,
            image_resolution: decode_resolution(raw.image, ImageResolution::new)?,
        })
    }
}

fn raw_sandbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSandbox> {
    Ok(RawSandbox {
        id: row.get(0)?,
        root: row.get(1)?,
        desired: row.get(2)?,
        actual: row.get(3)?,
        setup: (row.get(4)?, row.get(5)?),
        tools: (row.get(6)?, row.get(7)?),
        image: (row.get(8)?, row.get(9)?),
    })
}

fn sandbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SandboxRecord> {
    SandboxRecord::try_from(raw_sandbox_from_row(row)?).map_err(to_sql_conversion_error)
}

struct RawOperation {
    id: i64,
    sandbox_id: String,
    kind: String,
    status: String,
    error_code: Option<String>,
    error_details: Option<String>,
}

impl TryFrom<RawOperation> for OperationRecord {
    type Error = StoreError;
    fn try_from(raw: RawOperation) -> Result<Self, Self::Error> {
        Ok(Self {
            id: raw.id,
            sandbox_id: SandboxId::try_from(raw.sandbox_id)?,
            kind: OperationKind::from_db(&raw.kind)?,
            status: OperationStatus::from_db(&raw.status)?,
            error_code: raw.error_code,
            error_details: raw
                .error_details
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
        })
    }
}

fn raw_operation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawOperation> {
    Ok(RawOperation {
        id: row.get(0)?,
        sandbox_id: row.get(1)?,
        kind: row.get(2)?,
        status: row.get(3)?,
        error_code: row.get(4)?,
        error_details: row.get(5)?,
    })
}

fn operation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    OperationRecord::try_from(raw_operation_from_row(row)?).map_err(to_sql_conversion_error)
}

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationEvent> {
    let status: String = row.get(2)?;
    let details: Option<String> = row.get(3)?;
    Ok(OperationEvent {
        sequence: row.get(0)?,
        operation_id: row.get(1)?,
        status: OperationStatus::from_db(&status).map_err(to_sql_conversion_error)?,
        details: details
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(to_sql_conversion_error)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, StoreError> {
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Database)
}

fn validate_resolutions(sandbox: &SandboxRecord) -> Result<(), StoreError> {
    for version in [
        sandbox.setup_resolution.as_ref().map(|v| v.version),
        sandbox.tool_resolution.as_ref().map(|v| v.version),
        sandbox.image_resolution.as_ref().map(|v| v.version),
    ]
    .into_iter()
    .flatten()
    {
        if version == 0 {
            return Err(StoreError::CorruptData(
                "resolution version must be positive".to_owned(),
            ));
        }
    }
    Ok(())
}

fn encode_resolution<T>(record: Option<&T>) -> Result<(Option<u32>, Option<String>), StoreError>
where
    T: Resolution,
{
    record
        .map(|value| {
            Ok((
                Some(value.version()),
                Some(serde_json::to_string(value.details())?),
            ))
        })
        .unwrap_or(Ok((None, None)))
}

trait Resolution {
    fn version(&self) -> u32;
    fn details(&self) -> &Value;
}
macro_rules! impl_resolution {
    ($name:ident) => {
        impl Resolution for $name {
            fn version(&self) -> u32 {
                self.version
            }
            fn details(&self) -> &Value {
                &self.details
            }
        }
    };
}
impl_resolution!(SetupResolution);
impl_resolution!(ToolResolution);
impl_resolution!(ImageResolution);

fn decode_resolution<T>(
    raw: (Option<u32>, Option<String>),
    constructor: impl FnOnce(u32, Value) -> T,
) -> Result<Option<T>, StoreError> {
    match raw {
        (None, None) => Ok(None),
        (Some(version), Some(details)) if version > 0 => {
            Ok(Some(constructor(version, serde_json::from_str(&details)?)))
        }
        _ => Err(StoreError::CorruptData(
            "incomplete or invalid resolution record".to_owned(),
        )),
    }
}

fn validate_actual_transition(from: ActualState, to: ActualState) -> Result<(), StoreError> {
    let allowed = from == to
        || matches!(
            (from, to),
            (ActualState::Absent, ActualState::Creating)
                | (
                    ActualState::Creating,
                    ActualState::Running | ActualState::Stopped
                )
                | (
                    ActualState::Running,
                    ActualState::Stopped | ActualState::Destroying
                )
                | (
                    ActualState::Stopped,
                    ActualState::Running | ActualState::Destroying
                )
                | (ActualState::Destroying, ActualState::Absent)
        );
    if allowed {
        Ok(())
    } else {
        Err(StoreError::InvalidTransition {
            from: from.as_db().to_owned(),
            to: to.as_db().to_owned(),
        })
    }
}

fn validate_operation_transition(
    from: OperationStatus,
    to: OperationStatus,
) -> Result<(), StoreError> {
    if from == OperationStatus::Pending
        && matches!(to, OperationStatus::Completed | OperationStatus::Failed)
    {
        Ok(())
    } else {
        Err(StoreError::InvalidTransition {
            from: from.as_db().to_owned(),
            to: to.as_db().to_owned(),
        })
    }
}

fn validate_terminal_actual_transition(
    from: ActualState,
    to: ActualState,
    kind: OperationKind,
    status: OperationStatus,
) -> Result<(), StoreError> {
    if status == OperationStatus::Completed {
        return validate_actual_transition(from, to);
    }
    let rollback = matches!(
        (kind, from, to),
        (
            OperationKind::Create,
            ActualState::Creating,
            ActualState::Absent
        ) | (
            OperationKind::Destroy,
            ActualState::Destroying,
            ActualState::Running | ActualState::Stopped
        )
    );
    if rollback {
        Ok(())
    } else {
        validate_actual_transition(from, to)
    }
}

fn validate_v1_schema(connection: &Connection) -> Result<(), StoreError> {
    validate_table_columns(
        connection,
        "sandboxes",
        &[
            ("id", "TEXT", 1, 1),
            ("canonical_root", "TEXT", 1, 0),
            ("desired_state", "TEXT", 1, 0),
            ("actual_state", "TEXT", 1, 0),
            ("setup_resolution_version", "INTEGER", 0, 0),
            ("setup_resolution_details", "TEXT", 0, 0),
            ("tool_resolution_version", "INTEGER", 0, 0),
            ("tool_resolution_details", "TEXT", 0, 0),
            ("image_resolution_version", "INTEGER", 0, 0),
            ("image_resolution_details", "TEXT", 0, 0),
        ],
    )?;
    validate_table_columns(
        connection,
        "operations",
        &[
            ("id", "INTEGER", 0, 1),
            ("sandbox_id", "TEXT", 1, 0),
            ("kind", "TEXT", 1, 0),
            ("status", "TEXT", 1, 0),
            ("error_code", "TEXT", 0, 0),
            ("error_details", "TEXT", 0, 0),
        ],
    )?;
    validate_table_columns(
        connection,
        "operation_events",
        &[
            ("sequence", "INTEGER", 0, 1),
            ("operation_id", "INTEGER", 1, 0),
            ("status", "TEXT", 1, 0),
            ("details", "TEXT", 0, 0),
        ],
    )?;
    validate_foreign_keys(connection, "sandboxes", &[])?;
    validate_foreign_keys(
        connection,
        "operations",
        &[(
            "sandboxes",
            "sandbox_id",
            "id",
            "NO ACTION",
            "NO ACTION",
            "NONE",
        )],
    )?;
    validate_foreign_keys(
        connection,
        "operation_events",
        &[(
            "operations",
            "operation_id",
            "id",
            "NO ACTION",
            "NO ACTION",
            "NONE",
        )],
    )?;
    validate_unique_index(connection, "sandboxes", &["canonical_root"], None)?;
    validate_unique_index(
        connection,
        "operations",
        &["sandbox_id"],
        Some("one_pending_operation_per_sandbox"),
    )?;
    validate_trigger(connection, "operation_events_no_update", "before update")?;
    validate_trigger(connection, "operation_events_no_delete", "before delete")?;
    Ok(())
}

fn validate_table_columns(
    connection: &Connection,
    table: &str,
    expected: &[(&str, &str, i64, i64)],
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")
        .map_err(schema_error)?;
    let actual = statement
        .query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(schema_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(schema_error)?;
    let expected = expected
        .iter()
        .map(|(name, column_type, not_null, primary_key)| {
            (
                (*name).to_owned(),
                (*column_type).to_owned(),
                *not_null,
                *primary_key,
            )
        })
        .collect::<Vec<_>>();
    if actual == expected {
        Ok(())
    } else {
        Err(StoreError::SchemaMismatch(format!(
            "table {table} has unexpected columns"
        )))
    }
}

fn validate_foreign_keys(
    connection: &Connection,
    table: &str,
    expected: &[(&str, &str, &str, &str, &str, &str)],
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT \"table\", \"from\", \"to\", on_update, on_delete, \"match\" \
             FROM pragma_foreign_key_list(?1) ORDER BY id, seq",
        )
        .map_err(schema_error)?;
    let actual = statement
        .query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(schema_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(schema_error)?;
    let expected = expected
        .iter()
        .map(|values| {
            (
                values.0.to_owned(),
                values.1.to_owned(),
                values.2.to_owned(),
                values.3.to_owned(),
                values.4.to_owned(),
                values.5.to_owned(),
            )
        })
        .collect::<Vec<_>>();
    if actual == expected {
        Ok(())
    } else {
        Err(StoreError::SchemaMismatch(format!(
            "table {table} has an unexpected foreign key set"
        )))
    }
}

fn validate_unique_index(
    connection: &Connection,
    table: &str,
    columns: &[&str],
    required_name: Option<&str>,
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("SELECT name, \"unique\", partial FROM pragma_index_list(?1)")
        .map_err(schema_error)?;
    let indexes = statement
        .query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(schema_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(schema_error)?;
    for (name, unique, partial) in indexes {
        if !unique || required_name.is_some_and(|required| required != name) {
            continue;
        }
        if required_name.is_some() != partial {
            continue;
        }
        let mut index_statement = connection
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
            .map_err(schema_error)?;
        let actual_columns = index_statement
            .query_map([&name], |row| row.get::<_, String>(0))
            .map_err(schema_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(schema_error)?;
        if actual_columns == columns {
            if let Some(required) = required_name {
                let definition: String = connection
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
                        params!["index", required],
                        |row| row.get(0),
                    )
                    .map_err(schema_error)?;
                if normalize_sql(&definition)
                    != "create unique index one_pending_operation_per_sandbox on operations(sandbox_id) where status = 'pending'"
                {
                    continue;
                }
            }
            return Ok(());
        }
    }
    Err(StoreError::SchemaMismatch(format!(
        "table {table} is missing a required unique index"
    )))
}

fn validate_schema_version_constraint(connection: &Connection) -> Result<(), StoreError> {
    let definition: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
            params!["table", "schema_version"],
            |row| row.get(0),
        )
        .map_err(schema_error)?;
    if normalize_sql(&definition)
        == "create table schema_version ( singleton integer not null primary key check (singleton = 1), version integer not null )"
    {
        Ok(())
    } else {
        Err(StoreError::SchemaMismatch(
            "schema_version must enforce its singleton row".to_owned(),
        ))
    }
}

fn validate_trigger(
    connection: &Connection,
    name: &str,
    expected_action: &str,
) -> Result<(), StoreError> {
    let definition: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
            params!["trigger", name],
            |row| row.get(0),
        )
        .optional()
        .map_err(schema_error)?;
    let expected = format!(
        "create trigger {name} {expected_action} on operation_events begin select raise(abort, 'operation events are append-only'); end"
    );
    let valid = definition.is_some_and(|sql| normalize_sql(&sql) == expected);
    if valid {
        Ok(())
    } else {
        Err(StoreError::SchemaMismatch(format!(
            "missing or malformed trigger {name}"
        )))
    }
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn schema_query<T: rusqlite::types::FromSql>(
    connection: &Connection,
    sql: &str,
) -> Result<T, StoreError> {
    connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(schema_error)
}

fn schema_error(error: rusqlite::Error) -> StoreError {
    StoreError::SchemaMismatch(error.to_string())
}
fn to_sql_conversion_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

macro_rules! db_enum {
    ($type:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        impl $type {
            const fn as_db(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
            fn from_db(value: &str) -> Result<Self, StoreError> { match value { $($value => Ok(Self::$variant),)+ other => Err(StoreError::CorruptData(format!("invalid {} value {other}", stringify!($type)))) } }
        }
    };
}
db_enum!(DesiredState { Absent => "absent", Running => "running", Stopped => "stopped" });
db_enum!(ActualState { Absent => "absent", Creating => "creating", Running => "running", Stopped => "stopped", Destroying => "destroying" });
db_enum!(OperationKind { Create => "create", Apply => "apply", Start => "start", Stop => "stop", Destroy => "destroy", Reconcile => "reconcile" });
db_enum!(OperationStatus { Pending => "pending", Completed => "completed", Failed => "failed" });
