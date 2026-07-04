CREATE TABLE IF NOT EXISTS federation_dkg_setup_messages (
    phase TEXT NOT NULL,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (phase, member_id)
);

CREATE INDEX IF NOT EXISTS idx_federation_dkg_setup_member
ON federation_dkg_setup_messages (member_id);
