CREATE TABLE schema_version (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL
);

INSERT INTO schema_version (singleton, version) VALUES (1, 1);

CREATE TABLE sandboxes (
    id TEXT NOT NULL PRIMARY KEY,
    canonical_root TEXT NOT NULL UNIQUE,
    desired_state TEXT NOT NULL,
    actual_state TEXT NOT NULL,
    setup_resolution_version INTEGER,
    setup_resolution_details TEXT,
    tool_resolution_version INTEGER,
    tool_resolution_details TEXT,
    image_resolution_version INTEGER,
    image_resolution_details TEXT
);

CREATE TABLE operations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    error_code TEXT,
    error_details TEXT
);

CREATE TABLE operation_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id INTEGER NOT NULL REFERENCES operations(id),
    status TEXT NOT NULL,
    details TEXT
);

CREATE UNIQUE INDEX one_pending_operation_per_sandbox
ON operations(sandbox_id)
WHERE status = 'pending';

CREATE TRIGGER operation_events_no_update
BEFORE UPDATE ON operation_events
BEGIN
    SELECT RAISE(ABORT, 'operation events are append-only');
END;

CREATE TRIGGER operation_events_no_delete
BEFORE DELETE ON operation_events
BEGIN
    SELECT RAISE(ABORT, 'operation events are append-only');
END;
