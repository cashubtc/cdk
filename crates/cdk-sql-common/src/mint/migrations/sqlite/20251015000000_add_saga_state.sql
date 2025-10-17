-- Add saga_state table for persisting saga state
CREATE TABLE IF NOT EXISTS saga_state (
    operation_id TEXT PRIMARY KEY,
    operation_kind TEXT NOT NULL,
    state TEXT NOT NULL,
    blinded_secrets TEXT NOT NULL,
    input_ys TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_saga_state_operation_kind ON saga_state(operation_kind);
