CREATE TABLE IF NOT EXISTS federation_dkg_participant_states (
    ceremony_id TEXT PRIMARY KEY,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_dkg_participant_states_member
ON federation_dkg_participant_states (member_id);
