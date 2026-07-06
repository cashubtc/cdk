-- Add missing indexes for CTF and swap saga query paths.
CREATE INDEX IF NOT EXISTS idx_blind_signature_operation_id_only ON blind_signature(operation_id);
CREATE INDEX IF NOT EXISTS idx_proof_operation_id_only ON proof(operation_id);
CREATE INDEX IF NOT EXISTS idx_blind_signature_keyset_id ON blind_signature(keyset_id);
CREATE INDEX IF NOT EXISTS idx_proof_keyset_id ON proof(keyset_id);
CREATE INDEX IF NOT EXISTS idx_conditions_created_at ON conditions(created_at);
CREATE INDEX IF NOT EXISTS idx_conditional_keyset_active_created ON conditional_keyset(active, created_at);
