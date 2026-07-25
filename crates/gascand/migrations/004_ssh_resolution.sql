ALTER TABLE sandboxes ADD COLUMN ssh_resolution_version INTEGER;
ALTER TABLE sandboxes ADD COLUMN ssh_resolution_details TEXT;
UPDATE schema_version SET version = 4 WHERE singleton = 1;
