-- Saga-managed transactions use the canonical, hyphenless saga ID bytes as
-- their unique storage key.
-- Legacy transactions without a saga ID retain their proof-derived key.
UPDATE transactions
SET id = CAST(replace(saga_id, '-', '') AS BLOB)
WHERE saga_id IS NOT NULL;
