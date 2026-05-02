-- No-op: tags_json is created by the base 20260216 migration on fresh DBs.
-- This migration exists for Postgres compatibility (where IF NOT EXISTS
-- makes the ADD COLUMN idempotent). SQLite databases that lack tags_json
-- should be recreated from scratch.
SELECT 1;
