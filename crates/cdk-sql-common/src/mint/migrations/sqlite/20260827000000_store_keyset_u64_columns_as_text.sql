-- Store valid_from, valid_to and input_fee_ppk as text so the u64 values they
-- hold round-trip losslessly. SQLite has no ALTER COLUMN TYPE, so the table is
-- rebuilt; the INSERT ... SELECT applies TEXT affinity to existing rows.

CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    valid_from TEXT NOT NULL,
    valid_to TEXT,
    derivation_path TEXT NOT NULL,
    input_fee_ppk TEXT,
    derivation_path_index INTEGER,
    amounts TEXT,
    issuer_version TEXT
);

INSERT INTO keyset_new (id, unit, active, valid_from, valid_to, derivation_path, input_fee_ppk, derivation_path_index, amounts, issuer_version)
SELECT id, unit, active, valid_from, valid_to, derivation_path, input_fee_ppk, derivation_path_index, amounts, issuer_version
FROM keyset;

DROP TABLE keyset;

ALTER TABLE keyset_new RENAME TO keyset;

CREATE INDEX IF NOT EXISTS unit_index ON keyset(unit);
CREATE INDEX IF NOT EXISTS active_index ON keyset(active);
