-- Fix idx_melt_quote_request_lookup_id_and_kind if it was created on mint_quote.
DROP INDEX IF EXISTS idx_melt_quote_request_lookup_id_and_kind;

CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id_and_kind
ON melt_quote(request_lookup_id, request_lookup_id_kind);
