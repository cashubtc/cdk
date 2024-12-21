CREATE TABLE IF NOT EXISTS proof (
y BLOB PRIMARY KEY,
keyset_id TEXT NOT NULL,
secret TEXT NOT NULL,
c BLOB NOT NULL,
state TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS state_index ON proof(state);
CREATE INDEX IF NOT EXISTS secret_index ON proof(secret);


-- Keysets Table

CREATE TABLE IF NOT EXISTS keyset (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    valid_from INTEGER NOT NULL,
    valid_to INTEGER,
    derivation_path TEXT NOT NULL,
    max_order INTEGER NOT NULL,
    derivation_path_index INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS unit_index ON keyset(unit);
CREATE INDEX IF NOT EXISTS active_index ON keyset(active);


CREATE TABLE IF NOT EXISTS blind_signature (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    c BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS keyset_id_index ON blind_signature(keyset_id);


CREATE TABLE IF NOT EXISTS protected_endpoints (
    endpoint TEXT PRIMARY KEY,
    auth TEXT NOT NULL
);

