-- Fix: add tags_json column to conditions table.
-- The original 20260216 migration created a `description` column but the
-- current code expects `tags_json`. Migrates existing description values
-- into NIP-88 tag arrays.
ALTER TABLE conditions ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]';
UPDATE conditions SET tags_json = '[["description",' || json_quote(description) || ']]' WHERE description IS NOT NULL AND description != '' AND tags_json = '[]';
