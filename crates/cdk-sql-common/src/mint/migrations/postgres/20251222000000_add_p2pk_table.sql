REATE TABLE IF NOT EXISTS p2pk_signing_key (
    pubkey BYTEA PRIMARY KEY,
    derivation_index INTEGER NOT NULL,
    derivation_path TEXT NOT NULL,
    created_time BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT)
);