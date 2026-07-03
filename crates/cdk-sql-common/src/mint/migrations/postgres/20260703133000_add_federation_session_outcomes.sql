CREATE TABLE IF NOT EXISTS federation_session_outcomes (
    from_index BIGINT NOT NULL,
    signer_member_id INTEGER NOT NULL,
    latest_index BIGINT NOT NULL,
    operation_count BIGINT NOT NULL,
    outcome_json TEXT NOT NULL,
    PRIMARY KEY (from_index, signer_member_id)
);

CREATE INDEX IF NOT EXISTS idx_federation_session_outcomes_from_index
ON federation_session_outcomes (from_index);
