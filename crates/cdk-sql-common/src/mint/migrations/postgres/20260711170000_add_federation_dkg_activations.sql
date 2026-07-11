CREATE TABLE IF NOT EXISTS federation_dkg_activations (
    ceremony_id TEXT NOT NULL,
    member_id BIGINT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (ceremony_id, member_id)
);

CREATE INDEX IF NOT EXISTS idx_federation_dkg_activations_member
ON federation_dkg_activations (member_id);
