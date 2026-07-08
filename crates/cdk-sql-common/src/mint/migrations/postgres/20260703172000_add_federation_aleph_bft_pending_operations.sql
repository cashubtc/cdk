CREATE TABLE IF NOT EXISTS federation_aleph_bft_pending_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    session BIGINT NOT NULL,
    envelope_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_aleph_bft_pending_operations_session
ON federation_aleph_bft_pending_operations (session);
