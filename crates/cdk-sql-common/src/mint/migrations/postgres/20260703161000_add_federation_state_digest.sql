CREATE TABLE IF NOT EXISTS federation_state_digest (
    operation_index BIGINT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    order_digest TEXT NOT NULL,
    state_digest TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_state_digest_state
ON federation_state_digest (state_digest);
