-- Add partial unique index on melt_quote for consistency with PostgreSQL
-- Enforces at the database level that only one quote per request_lookup_id/kind can be PENDING or PAID
CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_paid_lookup_id
ON melt_quote(request_lookup_id, request_lookup_id_kind)
WHERE state IN ('PENDING', 'PAID');
