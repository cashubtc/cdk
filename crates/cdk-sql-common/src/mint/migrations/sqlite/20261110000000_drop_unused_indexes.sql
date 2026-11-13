-- Drop indexes that no query uses. Each one is maintained on every write to its
-- table without serving a lookup.

-- No query filters proofs by state without the primary key.
DROP INDEX IF EXISTS idx_proof_state_operation;

-- Melt quotes are never looked up by state alone.
DROP INDEX IF EXISTS melt_quote_state_index;
-- Prefix of idx_melt_quote_request_lookup_id_and_kind.
DROP INDEX IF EXISTS idx_melt_quote_request_lookup_id;

-- Covered by the UNIQUE constraint on mint_quote.request_lookup_id.
DROP INDEX IF EXISTS idx_mint_quote_request_lookup_id;
DROP INDEX IF EXISTS idx_mint_quote_request_lookup_id_and_kind;
-- No query filters or sorts mint quotes by these columns.
DROP INDEX IF EXISTS idx_mint_quote_created_time;
DROP INDEX IF EXISTS idx_mint_quote_expiry;

-- Covered by the UNIQUE constraint on mint_quote_payments.payment_id.
DROP INDEX IF EXISTS idx_mint_quote_payments_payment_id;

-- Prefix of the kv_store primary key.
DROP INDEX IF EXISTS idx_kv_store_namespaces;
-- No query filters or sorts by updated_time.
DROP INDEX IF EXISTS idx_kv_store_updated_time;

-- Only serve listing queries that the mint does not run.
DROP INDEX IF EXISTS idx_completed_operations_kind_time;
DROP INDEX IF EXISTS idx_completed_operations_time;
DROP INDEX IF EXISTS keyset_id_index;
