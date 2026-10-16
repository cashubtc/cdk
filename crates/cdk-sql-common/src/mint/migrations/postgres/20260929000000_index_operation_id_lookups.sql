-- Saga lookups filter proofs and blind signatures by operation_id alone, which
-- cannot use the (operation_kind, operation_id) indexes. No query filters on
-- both columns, so replace them with operation_id indexes.
DROP INDEX IF EXISTS idx_proof_operation_id;
DROP INDEX IF EXISTS idx_blind_sig_operation_id;

CREATE INDEX IF NOT EXISTS idx_proof_operation_id ON proof(operation_id);
CREATE INDEX IF NOT EXISTS idx_blind_sig_operation_id ON blind_signature(operation_id);
