CREATE TABLE IF NOT EXISTS federation_aleph_bft_backups (
    session BIGINT PRIMARY KEY NOT NULL,
    backup_bytes BYTEA NOT NULL
);
