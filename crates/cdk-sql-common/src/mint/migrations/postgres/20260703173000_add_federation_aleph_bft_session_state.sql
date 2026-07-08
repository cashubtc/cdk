CREATE TABLE IF NOT EXISTS federation_aleph_bft_session_state (
    session_key TEXT PRIMARY KEY,
    active_session BIGINT NOT NULL CHECK (active_session >= 0)
);
