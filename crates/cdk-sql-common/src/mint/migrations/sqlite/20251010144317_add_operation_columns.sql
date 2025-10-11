-- Add operation and operation_id columns to proof table
ALTER TABLE proof ADD COLUMN operation_kind TEXT;
ALTER TABLE proof ADD COLUMN operation_id TEXT;

-- Add operation and operation_id columns to blind_signature table
ALTER TABLE blind_signature ADD COLUMN operation_kind TEXT;
ALTER TABLE blind_signature ADD COLUMN operation_id TEXT;

CREATE INDEX idx_proof_state_operation ON proof(state, operation_kind);
CREATE INDEX idx_proof_operation_id ON proof(operation_kind, operation_id);
CREATE INDEX idx_blind_sig_operation_id ON blind_signature(operation_kind, operation_id);
