CREATE TABLE IF NOT EXISTS federation_aleph_bft_finalized_items (
    accepted_item_index BIGINT PRIMARY KEY NOT NULL,
    session BIGINT NOT NULL,
    unit_finalization_index BIGINT NOT NULL,
    unit_submitting_member INTEGER NOT NULL,
    unit_index BIGINT NOT NULL,
    item_index_in_unit BIGINT NOT NULL,
    operation_id TEXT,
    status TEXT NOT NULL,
    unit_body_hash TEXT NOT NULL,
    item_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_aleph_bft_finalized_items_session_unit
ON federation_aleph_bft_finalized_items (
    session,
    unit_submitting_member,
    unit_index
);

CREATE INDEX IF NOT EXISTS idx_federation_aleph_bft_finalized_items_operation_id
ON federation_aleph_bft_finalized_items (operation_id);
