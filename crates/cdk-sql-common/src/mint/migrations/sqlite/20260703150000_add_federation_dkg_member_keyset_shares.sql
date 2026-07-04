CREATE TABLE IF NOT EXISTS federation_dkg_member_keyset_shares (
    keyset_id TEXT PRIMARY KEY,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_dkg_member_keyset_shares_member
ON federation_dkg_member_keyset_shares (member_id);
