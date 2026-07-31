-- Add per-keyset operation count columns to support volume-based autorotate triggers
ALTER TABLE keyset_amounts ADD COLUMN issued_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE keyset_amounts ADD COLUMN redeemed_count INTEGER NOT NULL DEFAULT 0;

-- Backfill issued_count from existing blind signatures
UPDATE keyset_amounts SET issued_count = (
    SELECT COUNT(*) FROM blind_signature
    WHERE blind_signature.keyset_id = keyset_amounts.keyset_id AND c IS NOT NULL
);

-- Backfill redeemed_count from existing spent proofs
UPDATE keyset_amounts SET redeemed_count = (
    SELECT COUNT(*) FROM proof
    WHERE proof.keyset_id = keyset_amounts.keyset_id AND state = 'SPENT'
);
