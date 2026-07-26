ALTER TABLE sandboxes
    ADD COLUMN ssh_transport_enabled INTEGER
    CHECK (ssh_transport_enabled IS NULL OR ssh_transport_enabled IN (0, 1));

ALTER TABLE sandboxes
    ADD COLUMN ssh_transport_host_port INTEGER
    CHECK (
        ssh_transport_host_port IS NULL
        OR ssh_transport_host_port BETWEEN 1024 AND 65535
    );

UPDATE schema_version SET version = 5 WHERE singleton = 1;
