CREATE TABLE IF NOT EXISTS federation_dkg_transcript_signatures (
    transcript_hash TEXT NOT NULL,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (transcript_hash, member_id)
);

CREATE INDEX IF NOT EXISTS idx_federation_dkg_transcript_signatures_member
ON federation_dkg_transcript_signatures (member_id);
