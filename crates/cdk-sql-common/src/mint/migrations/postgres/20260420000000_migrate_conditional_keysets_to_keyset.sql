-- Migration: replace old `conditional_keysets` mapping table with the full
-- `conditional_keyset` table that mirrors the `keyset` schema plus CTF columns.
--
-- The original 20260216000000_add_conditions_tables migration was modified
-- in-place (commit 4d958587) but its name was already recorded in the
-- `migrations` table, so the updated DDL was never applied. This migration
-- creates the correct table for deployments that ran the original version.

-- Drop the old mapping table (created by the original version of
-- 20260216000000_add_conditions_tables).
DROP TABLE IF EXISTS conditional_keysets;

-- Create the full conditional_keyset table (matching the current codebase).
CREATE TABLE IF NOT EXISTS conditional_keyset (
    id                     TEXT    PRIMARY KEY,
    unit                   TEXT    NOT NULL,
    active                 BOOLEAN NOT NULL,
    valid_from             BIGINT  NOT NULL,
    valid_to               BIGINT,
    derivation_path        TEXT    NOT NULL,
    derivation_path_index  BIGINT,
    input_fee_ppk          BIGINT  NOT NULL DEFAULT 0,
    amounts                TEXT    NOT NULL,
    issuer_version         TEXT,

    condition_id           TEXT    NOT NULL,
    outcome_collection     TEXT    NOT NULL,
    outcome_collection_id  TEXT    NOT NULL,
    created_at             BIGINT  NOT NULL DEFAULT 0,

    FOREIGN KEY (condition_id) REFERENCES conditions(condition_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS conditional_keyset_active_per_collection
    ON conditional_keyset(outcome_collection_id)
    WHERE active = TRUE;

CREATE INDEX IF NOT EXISTS conditional_keyset_condition_id_idx
    ON conditional_keyset(condition_id);

CREATE INDEX IF NOT EXISTS conditional_keyset_outcome_collection_id_idx
    ON conditional_keyset(outcome_collection_id);

CREATE INDEX IF NOT EXISTS conditional_keyset_created_at_idx
    ON conditional_keyset(created_at);
