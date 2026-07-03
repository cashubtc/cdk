CREATE TABLE IF NOT EXISTS federation_journal_entries (
    operation_index BIGINT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    envelope_json TEXT NOT NULL,
    conflict_key_json TEXT,
    conflict_keys_json TEXT NOT NULL,
    previous_order_digest TEXT NOT NULL,
    order_digest TEXT NOT NULL,
    status TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_journal_operation_id
ON federation_journal_entries (operation_id);

CREATE TABLE IF NOT EXISTS federation_member_sequences (
    member_id INTEGER PRIMARY KEY,
    last_sequence BIGINT NOT NULL
);
