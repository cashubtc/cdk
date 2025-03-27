
CREATE TABLE IF NOT EXISTS kvac_coins (
    nullifier BLOB PRIMARY KEY,
    tag BLOB NOT NULL,
    mac BLOB NOT NULL,
    amount INTEGER NOT NULL,
    amount_blinding_factor BLOB NOT NULL,
    script TEXT DEFAULT NULL,
    script_blinding_factor BLOB DEFAULT NULL,
    mint_url TEXT NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED' ) ) NOT NULL,
    unit TEXT NOT NULL,
    keyset_id TEXT NOT NULL,
    issuance_proof TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kvac_keyset (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER NOT NULL,
    counter INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(mint_url) REFERENCES mint(mint_url) ON UPDATE CASCADE ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS kvac_key (
    id TEXT PRIMARY KEY,
    Cw BLOB NOT NULL,
    I BLOB NOT NULL
);