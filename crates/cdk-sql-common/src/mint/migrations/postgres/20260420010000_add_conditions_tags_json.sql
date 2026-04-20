-- Fix: ensure tags_json column exists on conditions table.
-- On fresh DBs (where 20260216 already creates tags_json) this is a no-op.
-- On old DBs (where 20260216 created description instead) this adds the column.
-- Data migration from description is not needed since staging DB was reset.
ALTER TABLE conditions ADD COLUMN IF NOT EXISTS tags_json TEXT NOT NULL DEFAULT '[]';
