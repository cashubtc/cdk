-- Saga-managed transactions use the canonical, hyphenless saga ID bytes as
-- their unique storage key. Supabase stores those bytes as a hex string.
-- Legacy transactions without a saga ID retain their proof-derived key.
UPDATE transactions
SET id = encode(convert_to(replace(saga_id, '-', ''), 'UTF8'), 'hex')
WHERE saga_id IS NOT NULL;
