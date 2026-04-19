-- Migration: fix schema drift from in-place modification of
-- 20260216000000_add_conditions_tables (commit 4d958587).
--
-- The original migration was already recorded in the `migrations` table,
-- so its updated DDL was never applied. This migration brings existing
-- deployments in line with the current codebase.
--
-- Changes:
--   1. conditions.description -> conditions.tags_json (column rename)
--   2. conditional_keysets (mapping table) -> conditional_keyset (full keyset table)

-- 1. Add tags_json column and migrate data from description.
--    Leave description in place (SQLite DROP COLUMN requires 3.35+).
ALTER TABLE conditions ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]';
UPDATE conditions SET tags_json = '[["description",' || json_quote(description) || ']]' WHERE description IS NOT NULL AND description != '';

-- 2. Drop the old mapping table (created by the original version of
--    20260216000000_add_conditions_tables).
DROP TABLE IF EXISTS conditional_keysets;

-- 3. Create the full conditional_keyset table (matching the current codebase).
CREATE TABLE IF NOT EXISTS conditional_keyset (
    id                     TEXT    PRIMARY KEY,
    unit                   TEXT    NOT NULL,
    active                 BOOL    NOT NULL,
    valid_from             INTEGER NOT NULL,
    valid_to               INTEGER,
    derivation_path        TEXT    NOT NULL,
    derivation_path_index  INTEGER,
    input_fee_ppk          INTEGER NOT NULL DEFAULT 0,
    amounts                TEXT    NOT NULL,
    issuer_version         TEXT,

    condition_id           TEXT    NOT NULL,
    outcome_collection     TEXT    NOT NULL,
    outcome_collection_id  TEXT    NOT NULL,
    created_at             INTEGER NOT NULL DEFAULT 0,

    FOREIGN KEY (condition_id) REFERENCES conditions(condition_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS conditional_keyset_active_per_collection
    ON conditional_keyset(outcome_collection_id)
    WHERE active = 1;

CREATE INDEX IF NOT EXISTS conditional_keyset_condition_id_idx
    ON conditional_keyset(condition_id);

CREATE INDEX IF NOT EXISTS conditional_keyset_outcome_collection_id_idx
    ON conditional_keyset(outcome_collection_id);

CREATE INDEX IF NOT EXISTS conditional_keyset_created_at_idx
    ON conditional_keyset(created_at);
