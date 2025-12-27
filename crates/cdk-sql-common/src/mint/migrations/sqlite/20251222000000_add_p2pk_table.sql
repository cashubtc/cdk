CREATE TABLE IF NOT EXISTS p2pk_signing_key (
    pubkey BLOB PRIMARY KEY,
    derivation_index INTEGER NOT NULL,
    derivation_path TEXT NOT NULL,
    created_time INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);