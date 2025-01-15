CREATE TABLE IF NOT EXISTS kvac_nullifiers (
    nullifier BLOB PRIMARY KEY,
    keyset_id TEXT NOT NULL,
    script BLOB DEFAULT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'PENDING' ) ) NOT NULL
);

CREATE TABLE IF NOT EXISTS kvac_signed_tags (
    t_tag BLOB PRIMARY KEY,
    mac BLOB NOT NULL,
    keyset_id TEXT NOT NULL
);
