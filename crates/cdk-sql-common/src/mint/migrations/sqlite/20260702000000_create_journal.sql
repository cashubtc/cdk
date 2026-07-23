-- Append-only journal of entity creations (snapshots) and mutations (deltas).
-- A row is identified by the compound (entity, record) key; replaying one
-- row's events in id order reconstructs its current state.
CREATE TABLE IF NOT EXISTS journal (
    id          INTEGER PRIMARY KEY,   -- Snowflake i64, time-sortable
    entity      INTEGER NOT NULL,      -- Entity enum discriminant (source table)
    record      TEXT    NOT NULL,      -- primary key within the entity
    event       BLOB    NOT NULL,      -- serialized Event (Snapshot | Delta)
    created_at  INTEGER NOT NULL       -- unix seconds at insert time
);

CREATE INDEX IF NOT EXISTS idx_journal_entity_record ON journal(entity, record, id);
