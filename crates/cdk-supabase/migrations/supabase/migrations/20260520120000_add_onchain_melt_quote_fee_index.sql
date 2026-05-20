-- NUT-30 introduced server-assigned `fee_index` as the selector for
-- onchain melt fee options. Persist it with the existing wallet-side
-- onchain quote metadata so resumed melts can echo the selected option.

ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS estimated_blocks INTEGER;
ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS fee_index INTEGER;

-- Bump schema version
INSERT INTO schema_info (key, value) VALUES ('schema_version', '6')
ON CONFLICT (key) DO UPDATE SET value = '6';
