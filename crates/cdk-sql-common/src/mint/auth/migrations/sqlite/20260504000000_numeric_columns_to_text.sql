-- Migrate amount columns to TEXT
-- u64 amounts cannot be faithfully represented as SQLite INTEGER (signed i64)

-- ============================================================================
-- blind_signature
-- ============================================================================
CREATE TABLE blind_signature_new (
    blinded_message BLOB PRIMARY KEY,
    amount TEXT NOT NULL,
    keyset_id TEXT NOT NULL,
    c BLOB NOT NULL
);

INSERT INTO blind_signature_new (blinded_message, amount, keyset_id, c)
SELECT blinded_message, CAST(amount AS TEXT), keyset_id, c
FROM blind_signature;

DROP TABLE blind_signature;
ALTER TABLE blind_signature_new RENAME TO blind_signature;

CREATE INDEX IF NOT EXISTS keyset_id_index ON blind_signature(keyset_id);
