
CREATE TABLE IF NOT EXISTS kvac_coins (
    t BLOB PRIMARY KEY,
    V BLOB NOT NULL,
    amount INTEGER NOT NULL,
    r_a BLOB NOT NULL,
    script BLOB DEFAULT NULL,
    r_s BLOB DEFAULT NULL,
    mint_url TEXT NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED' ) ) NOT NULL,
    unit TEXT NOT NULL,
    keyset_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kvac_null_coins (
    t BLOB PRIMARY KEY,
    V BLOB NOT NULL,
    r_a BLOB NOT NULL,
    script BLOB DEFAULT NULL,
    r_s BLOB DEFAULT NULL,
    mint_url TEXT NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED' ) ) NOT NULL,
    unit TEXT NOT NULL,
    keyset_id TEXT NOT NULL
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
    I BLOB NOT NULL,
);