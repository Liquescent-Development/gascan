ALTER TABLE sandboxes ADD COLUMN storage_resolution_version INTEGER;
ALTER TABLE sandboxes ADD COLUMN storage_resolution_details TEXT;
UPDATE schema_version SET version = 3 WHERE singleton = 1;
