CREATE TABLE IF NOT EXISTS kvac_nullifiers (
    nullifier BLOB PRIMARY KEY,
    keyset_id TEXT NOT NULL,
    script BLOB DEFAULT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'PENDING' ) ) NOT NULL
);

CREATE INDEX IF NOT EXISTS kvac_state_index ON kvac_nullifiers(state);

CREATE TABLE IF NOT EXISTS kvac_signed_tags (
    u BLOB PRIMARY KEY,
    mac BLOB NOT NULL,
    keyset_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kvac_keyset (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    valid_from INTEGER NOT NULL,
    valid_to INTEGER,
    derivation_path TEXT NOT NULL,
    derivation_path_index INTEGER NOT NULL
);
