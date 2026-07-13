-- Track the last quote update timestamp so wallet quote responses cannot
-- move amount_paid or amount_issued backwards after stale notifications.

ALTER TABLE mint_quote ADD COLUMN IF NOT EXISTS updated_at BIGINT NOT NULL DEFAULT 0;

INSERT INTO schema_info (key, value) VALUES ('schema_version', '8')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
