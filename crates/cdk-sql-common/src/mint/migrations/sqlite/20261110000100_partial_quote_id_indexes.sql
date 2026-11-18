-- Only melts set quote_id on proofs, blind signatures, and sagas. Index just
-- those rows so swaps do not maintain entries for NULL quote ids.
DROP INDEX IF EXISTS blind_signature_quote_id_index;
CREATE INDEX IF NOT EXISTS blind_signature_quote_id_index
ON blind_signature(quote_id)
WHERE quote_id IS NOT NULL;

DROP INDEX IF EXISTS idx_saga_state_quote_id;
CREATE INDEX IF NOT EXISTS idx_saga_state_quote_id
ON saga_state(quote_id)
WHERE quote_id IS NOT NULL;

-- Melt finalization looks up input proofs by quote_id, which had no index.
CREATE INDEX IF NOT EXISTS idx_proof_quote_id
ON proof(quote_id)
WHERE quote_id IS NOT NULL;
