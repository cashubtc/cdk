ALTER TABLE federation_aleph_bft_backups
RENAME TO federation_aleph_bft_backups_legacy;

CREATE TABLE federation_aleph_bft_backups (
    session BIGINT NOT NULL,
    chunk_index BIGINT NOT NULL CHECK (chunk_index >= 0),
    backup_bytes BYTEA NOT NULL,
    PRIMARY KEY (session, chunk_index)
);

INSERT INTO federation_aleph_bft_backups
    (session, chunk_index, backup_bytes)
SELECT session, 0, backup_bytes
FROM federation_aleph_bft_backups_legacy;

DROP TABLE federation_aleph_bft_backups_legacy;
