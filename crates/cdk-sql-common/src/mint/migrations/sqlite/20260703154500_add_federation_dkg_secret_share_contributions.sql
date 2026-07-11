CREATE TABLE IF NOT EXISTS federation_dkg_secret_share_contributions (
    ceremony_id TEXT NOT NULL,
    sender_id INTEGER NOT NULL,
    receiver_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (ceremony_id, sender_id, receiver_id)
);

CREATE INDEX IF NOT EXISTS idx_federation_dkg_secret_share_contributions_receiver
ON federation_dkg_secret_share_contributions (ceremony_id, receiver_id);
