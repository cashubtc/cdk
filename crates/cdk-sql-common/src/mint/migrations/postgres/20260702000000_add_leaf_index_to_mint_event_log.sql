-- Zero-based Merkle tree position, assigned by the single transparency-log
-- appender when it folds a committed row into the tree — not by the INSERT
-- that created the row. Decoupling the tree position from the auto-increment
-- row id (`seq`) means gaps burned by rolled-back transactions (permanent on
-- Postgres, where sequence values survive a rollback) can never stall tree
-- advancement: the appender simply numbers the committed rows it observes,
-- densely, in `seq` order. NULL until the appender has indexed the row.
-- Once assigned, a row's leaf_index is never changed.
ALTER TABLE mint_event_log ADD COLUMN leaf_index BIGINT;

CREATE UNIQUE INDEX idx_mint_event_log_leaf_index ON mint_event_log(leaf_index);
