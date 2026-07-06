-- Add missing indexes for CTF and swap saga query paths.
-- These columns are queried in WHERE clauses but only had composite indexes
-- where the column was the second element, making them unusable for single-column queries.

-- Swap saga: blind_signature and proof queries by operation_id alone
-- (existing index is (operation_kind, operation_id) — can't be used without leading column)
CREATE INDEX IF NOT EXISTS idx_blind_signature_operation_id_only ON blind_signature(operation_id);
CREATE INDEX IF NOT EXISTS idx_proof_operation_id_only ON proof(operation_id);

-- Keyset-scoped lookups for restore/export
CREATE INDEX IF NOT EXISTS idx_blind_signature_keyset_id ON blind_signature(keyset_id);
CREATE INDEX IF NOT EXISTS idx_proof_keyset_id ON proof(keyset_id);

-- Conditions: unfiltered listing by created_at
-- (existing index is (attestation_status, created_at) — can't be used without status filter)
CREATE INDEX IF NOT EXISTS idx_conditions_created_at ON conditions(created_at);

-- Conditional keyset: active-filtered pagination
CREATE INDEX IF NOT EXISTS idx_conditional_keyset_active_created ON conditional_keyset(active, created_at);
