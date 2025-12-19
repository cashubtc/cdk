-- Remove unique constraint on request_lookup_id for melt_quote
-- This allows multiple melt quotes for the same payment request
-- The constraint that only one can be pending at a time is enforced in application logic

-- Drop the unique index on request_lookup_id
DROP INDEX IF EXISTS unique_request_lookup_id_melt;

-- Create a non-unique index for lookup performance
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id ON melt_quote(request_lookup_id);
