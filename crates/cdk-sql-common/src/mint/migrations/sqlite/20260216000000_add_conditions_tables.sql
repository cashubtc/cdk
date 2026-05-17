-- NUT-CTF: Conditional tokens - conditions table
CREATE TABLE IF NOT EXISTS conditions (
    condition_id TEXT PRIMARY KEY,
    threshold INTEGER NOT NULL DEFAULT 1,
    tags_json TEXT NOT NULL DEFAULT '[]',
    announcements_json TEXT NOT NULL,
    attestation_status TEXT NOT NULL DEFAULT 'pending',
    winning_outcome TEXT,
    attested_at INTEGER,
    created_at INTEGER NOT NULL
);

-- NUT-CTF: Conditional tokens - condition partitions table
CREATE TABLE IF NOT EXISTS condition_partitions (
    condition_id TEXT NOT NULL,
    partition_json TEXT NOT NULL,
    collateral TEXT NOT NULL,
    parent_collection_id TEXT NOT NULL DEFAULT '0000000000000000000000000000000000000000000000000000000000000000',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (condition_id, partition_json),
    FOREIGN KEY (condition_id) REFERENCES conditions(condition_id)
);

-- NUT-CTF: Conditional tokens - conditional keysets table
--
-- Standalone table mirroring `keyset` schema plus CTF-specific columns.
-- Conditional keysets live here and are NOT written to `keyset`, which keeps
-- the HashMap<CurrencyUnit, Id> collapse inside `reload_keys_from_db` from
-- clobbering the primary non-conditional keyset for each unit.
--
-- Active semantics: at most one active keyset per outcome_collection_id, enforced
-- by the partial unique index below. Regular `keyset` table keeps its original
-- "one active per unit" invariant untouched.
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

-- NEW invariant: at most one active keyset per outcome collection.
-- Nothing prevents multiple collections from each having their own active keyset.
CREATE UNIQUE INDEX IF NOT EXISTS conditional_keyset_active_per_collection
    ON conditional_keyset(outcome_collection_id)
    WHERE active = 1;

CREATE INDEX IF NOT EXISTS conditional_keyset_condition_id_idx
    ON conditional_keyset(condition_id);

CREATE INDEX IF NOT EXISTS conditional_keyset_outcome_collection_id_idx
    ON conditional_keyset(outcome_collection_id);

-- Listing and cursor-pagination queries all ORDER BY created_at ASC and
-- filter on `created_at > :since`.
CREATE INDEX IF NOT EXISTS conditional_keyset_created_at_idx
    ON conditional_keyset(created_at);
