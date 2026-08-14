-- Outbox table for the cross-instance SQL polling pub/sub bus.
-- Each row is one published event; peers poll rows newer than their cursor.
CREATE TABLE IF NOT EXISTS pubsub_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    origin TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_time INTEGER NOT NULL
);

-- Prune scans by age.
CREATE INDEX IF NOT EXISTS idx_pubsub_outbox_created_time
ON pubsub_outbox (created_time);
