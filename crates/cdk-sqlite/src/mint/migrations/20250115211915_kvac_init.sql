CREATE TABLE IF NOT EXISTS kvac_nullifiers (
    nullifier BLOB PRIMARY KEY,
    keyset_id TEXT NOT NULL,
    quote_id TEXT NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'PENDING', 'UNSPENT' ) ) NOT NULL
);

CREATE INDEX IF NOT EXISTS kvac_state_index ON kvac_nullifiers(state);

CREATE TABLE IF NOT EXISTS kvac_signed_tags (
    t BLOB PRIMARY KEY,
    V BLOB NOT NULL,
    keyset_id TEXT NOT NULL,
    quote_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kvac_keyset (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    valid_from INTEGER NOT NULL,
    valid_to INTEGER,
    derivation_path TEXT NOT NULL,
    derivation_path_index INTEGER NOT NULL,
    input_fee_ppk INTEGER NOT NULL
);
