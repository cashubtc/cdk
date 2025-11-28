-- Remove unique constraint on request_lookup_id for melt_quote
-- This allows multiple melt quotes for the same payment request
-- The constraint that only one can be PENDING or PAID at a time is enforced by a partial unique index

-- Drop the unique index on request_lookup_id
DROP INDEX IF EXISTS unique_request_lookup_id_melt;

-- Create a non-unique index for lookup performance
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id ON melt_quote(request_lookup_id);

-- Create a partial unique index to enforce that only one quote per lookup_id can be PENDING or PAID
-- This provides database-level enforcement of the constraint
CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_paid_lookup_id
ON melt_quote(request_lookup_id)
WHERE state IN ('PENDING', 'PAID');
