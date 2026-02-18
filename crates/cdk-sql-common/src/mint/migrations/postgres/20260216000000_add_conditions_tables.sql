-- NUT-28: Conditional tokens - conditions table
CREATE TABLE IF NOT EXISTS conditions (
    condition_id TEXT PRIMARY KEY,
    threshold INTEGER NOT NULL DEFAULT 1,
    description TEXT NOT NULL DEFAULT '',
    announcements_json TEXT NOT NULL,
    attestation_status TEXT NOT NULL DEFAULT 'pending',
    winning_outcome TEXT,
    attested_at BIGINT,
    created_at BIGINT NOT NULL
);

-- NUT-28: Conditional tokens - condition partitions table
CREATE TABLE IF NOT EXISTS condition_partitions (
    condition_id TEXT NOT NULL,
    partition_json TEXT NOT NULL,
    collateral TEXT NOT NULL,
    parent_collection_id TEXT NOT NULL DEFAULT '0000000000000000000000000000000000000000000000000000000000000000',
    created_at BIGINT NOT NULL,
    PRIMARY KEY (condition_id, partition_json),
    FOREIGN KEY (condition_id) REFERENCES conditions(condition_id)
);

-- NUT-28: Conditional tokens - conditional keyset mapping
CREATE TABLE IF NOT EXISTS conditional_keysets (
    condition_id TEXT NOT NULL,
    outcome_collection TEXT NOT NULL,
    outcome_collection_id TEXT NOT NULL,
    keyset_id TEXT NOT NULL,
    PRIMARY KEY (condition_id, outcome_collection),
    FOREIGN KEY (condition_id) REFERENCES conditions(condition_id)
);

CREATE INDEX IF NOT EXISTS idx_conditional_keysets_keyset_id ON conditional_keysets(keyset_id);
