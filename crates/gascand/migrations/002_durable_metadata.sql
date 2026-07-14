ALTER TABLE sandboxes ADD COLUMN updated_at_millis INTEGER NOT NULL DEFAULT 0;
UPDATE sandboxes SET updated_at_millis = unixepoch() * 1000;

ALTER TABLE operation_events ADD COLUMN error_code TEXT;
ALTER TABLE operation_events ADD COLUMN timestamp_millis INTEGER NOT NULL DEFAULT 0;
UPDATE operation_events SET timestamp_millis = unixepoch() * 1000;

UPDATE schema_version SET version = 2 WHERE singleton = 1;
