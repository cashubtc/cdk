-- Add per-keyset operation count columns to support volume-based autorotate triggers
ALTER TABLE keyset_amounts ADD COLUMN issued_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE keyset_amounts ADD COLUMN redeemed_count BIGINT NOT NULL DEFAULT 0;

-- Backfill issued_count from existing blind signatures
UPDATE keyset_amounts SET issued_count = sub.cnt
FROM (
    SELECT keyset_id, COUNT(*) AS cnt FROM blind_signature WHERE c IS NOT NULL GROUP BY keyset_id
) sub
WHERE keyset_amounts.keyset_id = sub.keyset_id;

-- Backfill redeemed_count from existing spent proofs
UPDATE keyset_amounts SET redeemed_count = sub.cnt
FROM (
    SELECT keyset_id, COUNT(*) AS cnt FROM proof WHERE state = 'SPENT' GROUP BY keyset_id
) sub
WHERE keyset_amounts.keyset_id = sub.keyset_id;
