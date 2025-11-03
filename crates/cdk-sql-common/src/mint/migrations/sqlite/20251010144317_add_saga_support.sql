-- Add operation and operation_id columns to proof table
ALTER TABLE proof ADD COLUMN operation_kind TEXT;
ALTER TABLE proof ADD COLUMN operation_id TEXT;

-- Add operation and operation_id columns to blind_signature table
ALTER TABLE blind_signature ADD COLUMN operation_kind TEXT;
ALTER TABLE blind_signature ADD COLUMN operation_id TEXT;

CREATE INDEX idx_proof_state_operation ON proof(state, operation_kind);
CREATE INDEX idx_proof_operation_id ON proof(operation_kind, operation_id);
CREATE INDEX idx_blind_sig_operation_id ON blind_signature(operation_kind, operation_id);

-- Add saga_state table for persisting saga state
CREATE TABLE IF NOT EXISTS saga_state (
    operation_id TEXT PRIMARY KEY,
    operation_kind TEXT NOT NULL,
    state TEXT NOT NULL,
    blinded_secrets TEXT NOT NULL,
    input_ys TEXT NOT NULL,
    quote_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_saga_state_operation_kind ON saga_state(operation_kind);
CREATE INDEX IF NOT EXISTS idx_saga_state_quote_id ON saga_state(quote_id);
