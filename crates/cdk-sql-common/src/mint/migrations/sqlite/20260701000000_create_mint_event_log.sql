-- Append-only transparency event log (docs/adr/0001-append-only-transparency-log.md).
--
-- `seq` is a stable, gap-tolerant total order: an aborted insert's `seq`
-- value is never reused and never appears, which is fine — the log's
-- consumers (the checkpoint publisher, playback tooling) treat `seq` as an
-- ordering key, not a strict "no missing values" counter.
CREATE TABLE mint_event_log (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    op SMALLINT NOT NULL,
    payload BLOB NOT NULL,
    leaf_hash BLOB NOT NULL,
    created_time BIGINT NOT NULL
);

CREATE INDEX idx_mint_event_log_entity ON mint_event_log(entity_type, entity_id, seq);
