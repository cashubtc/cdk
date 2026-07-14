ALTER TABLE federation_aleph_bft_backups
RENAME TO federation_aleph_bft_backups_legacy;

CREATE TABLE federation_aleph_bft_backups (
    session INTEGER NOT NULL,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    backup_bytes BLOB NOT NULL,
    PRIMARY KEY (session, chunk_index)
);

INSERT INTO federation_aleph_bft_backups
    (session, chunk_index, backup_bytes)
SELECT session, 0, backup_bytes
FROM federation_aleph_bft_backups_legacy;

DROP TABLE federation_aleph_bft_backups_legacy;
