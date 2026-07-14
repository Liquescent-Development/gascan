use camino::Utf8PathBuf;
use gascan_core::sandbox::{SandboxId, SandboxIdError};
use rusqlite::{Connection, ErrorCode, OptionalExtension, Transaction, params};
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
    #[error("conflicting sandbox identity: {0}")]
    Conflict(String),
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
        let transaction = connection.transaction()?;
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
        let transaction = connection.transaction()?;
        put_sandbox_in(&transaction, sandbox)?;
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
        let transaction = connection.transaction()?;
        let operation =
            load_operation(&transaction, id)?.ok_or(StoreError::OperationNotFound(id))?;
        validate_operation_transition(operation.status, status)?;
        let current_state: String = transaction.query_row(
            "SELECT actual_state FROM sandboxes WHERE id = ?1",
            [operation.sandbox_id.as_str()],
            |row| row.get(0),
        )?;
        let current_state = ActualState::from_db(&current_state)?;
        validate_actual_transition(current_state, actual_state)?;
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
        let version =
            connection.query_row("SELECT version FROM schema_version", [], |row| row.get(0))?;
        return if version == SCHEMA_VERSION {
            Ok(())
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
    let transaction = connection.transaction()?;
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
            return Err(StoreError::Conflict(format!(
                "sandbox ID {} already belongs to {}",
                sandbox.id, existing.canonical_root
            )));
        }
        validate_actual_transition(existing.actual_state, sandbox.actual_state)?;
    }
    let setup = encode_resolution(sandbox.setup_resolution.as_ref())?;
    let tools = encode_resolution(sandbox.tool_resolution.as_ref())?;
    let image = encode_resolution(sandbox.image_resolution.as_ref())?;
    let result = transaction.execute(
        "INSERT INTO sandboxes (id, canonical_root, desired_state, actual_state, setup_resolution_version, setup_resolution_details, tool_resolution_version, tool_resolution_details, image_resolution_version, image_resolution_details) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(id) DO UPDATE SET desired_state = excluded.desired_state, actual_state = excluded.actual_state, setup_resolution_version = excluded.setup_resolution_version, setup_resolution_details = excluded.setup_resolution_details, tool_resolution_version = excluded.tool_resolution_version, tool_resolution_details = excluded.tool_resolution_details, image_resolution_version = excluded.image_resolution_version, image_resolution_details = excluded.image_resolution_details",
        params![sandbox.id.as_str(), sandbox.canonical_root.as_str(), sandbox.desired_state.as_db(), sandbox.actual_state.as_db(), setup.0, setup.1, tools.0, tools.1, image.0, image.1],
    );
    match result {
        Ok(_) => Ok(()),
        Err(error) if is_constraint(&error) => Err(StoreError::Conflict(error.to_string())),
        Err(error) => Err(StoreError::Database(error)),
    }
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

fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::ConstraintViolation)
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
