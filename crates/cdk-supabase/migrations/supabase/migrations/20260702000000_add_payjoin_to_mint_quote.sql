ALTER TABLE mint_quote ADD COLUMN IF NOT EXISTS payjoin JSONB;
ALTER TABLE melt_quote ADD COLUMN IF NOT EXISTS payjoin JSONB;

-- Bump schema version
INSERT INTO schema_info (key, value) VALUES ('schema_version', '8')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
