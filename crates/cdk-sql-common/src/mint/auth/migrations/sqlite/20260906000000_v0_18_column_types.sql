-- Blind signature amounts are u64 on the wire, so they get the same eight big
-- endian bytes as the mint's own, converted by the u64 function the driver
-- registers. SQLite has no ALTER COLUMN and only a BLOB column stores the bytes
-- untouched, so the table is rebuilt.
CREATE TABLE blind_signature_new (
    blinded_message BLOB PRIMARY KEY,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    keyset_id TEXT NOT NULL,
    c BLOB NOT NULL
);

INSERT INTO blind_signature_new
SELECT blinded_message, u64(amount), keyset_id, c
FROM blind_signature;

DROP TABLE blind_signature;
ALTER TABLE blind_signature_new RENAME TO blind_signature;

CREATE INDEX keyset_id_index ON blind_signature(keyset_id);
