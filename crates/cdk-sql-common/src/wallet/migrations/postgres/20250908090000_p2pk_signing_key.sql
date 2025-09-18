-- Store P2PK signing keys for automatic signing on receive
CREATE TABLE IF NOT EXISTS p2pk_signing_key (
    pubkey BYTEA PRIMARY KEY,
    secret_key BYTEA NOT NULL,
    created_time BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT)
);

