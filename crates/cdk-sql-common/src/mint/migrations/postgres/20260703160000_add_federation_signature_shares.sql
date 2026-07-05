CREATE TABLE IF NOT EXISTS federation_signature_shares (
    operation_id TEXT NOT NULL,
    output_index INTEGER NOT NULL,
    member_id INTEGER NOT NULL,
    blinded_message BYTEA NOT NULL,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    signature_share BYTEA NOT NULL,
    share_json TEXT NOT NULL,
    PRIMARY KEY (operation_id, output_index, member_id)
);

CREATE INDEX IF NOT EXISTS idx_federation_signature_shares_operation_output
ON federation_signature_shares (operation_id, output_index);

CREATE INDEX IF NOT EXISTS idx_federation_signature_shares_blinded_message
ON federation_signature_shares (blinded_message);
