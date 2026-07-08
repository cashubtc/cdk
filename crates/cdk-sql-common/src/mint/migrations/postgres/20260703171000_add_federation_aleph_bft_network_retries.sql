CREATE TABLE IF NOT EXISTS federation_aleph_bft_network_retries (
    retry_id TEXT PRIMARY KEY NOT NULL,
    session BIGINT NOT NULL,
    request_json TEXT NOT NULL,
    peer_member_ids_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_aleph_bft_network_retries_session
ON federation_aleph_bft_network_retries (session);
