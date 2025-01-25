CREATE TABLE IF NOT EXISTS kvac_nullifiers (
    nullifier BLOB PRIMARY KEY,
    keyset_id TEXT NOT NULL,
    quote_id TEXT DEFAULT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'PENDING', 'UNSPENT' ) ) NOT NULL
);

CREATE INDEX IF NOT EXISTS kvac_state_index ON kvac_nullifiers(state);

CREATE TABLE IF NOT EXISTS kvac_issued_macs (
    t BLOB PRIMARY KEY,
    V BLOB NOT NULL,
    amount_commitment BLOB NOT NULL,
    script_commitment BLOB NOT NULL,
    keyset_id TEXT NOT NULL,
    quote_id TEXT DEFAULT NULL,
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
