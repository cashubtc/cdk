CREATE TABLE IF NOT EXISTS federation_aleph_bft_backups (
    session INTEGER PRIMARY KEY NOT NULL,
    backup_bytes BLOB NOT NULL
);
