-- Store P2PK signing keys for automatic signing on receive
CREATE TABLE IF NOT EXISTS p2pk_signing_key (
    pubkey BLOB PRIMARY KEY,
    secret_key BLOB NOT NULL,
    created_time INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

