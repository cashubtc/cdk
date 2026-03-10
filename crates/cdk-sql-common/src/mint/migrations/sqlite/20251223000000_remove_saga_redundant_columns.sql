-- Remove blinded_secrets and input_ys columns from saga_state table
-- These values can be looked up from proof and blind_signature tables using operation_id

-- SQLite doesn't support DROP COLUMN directly, so we need to recreate the table

-- Step 1: Create new table without the redundant columns
CREATE TABLE IF NOT EXISTS saga_state_new (
    operation_id TEXT PRIMARY KEY,
    operation_kind TEXT NOT NULL,
    state TEXT NOT NULL,
    quote_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Step 2: Copy data from old table to new table
INSERT INTO saga_state_new (operation_id, operation_kind, state, quote_id, created_at, updated_at)
SELECT operation_id, operation_kind, state, quote_id, created_at, updated_at
FROM saga_state;

-- Step 3: Drop old table
DROP TABLE saga_state;

-- Step 4: Rename new table to original name
ALTER TABLE saga_state_new RENAME TO saga_state;

-- Step 5: Recreate indexes
CREATE INDEX IF NOT EXISTS idx_saga_state_operation_kind ON saga_state(operation_kind);
CREATE INDEX IF NOT EXISTS idx_saga_state_quote_id ON saga_state(quote_id);
