-- Match the database invariant to the code's payment identifier identity:
-- (request_lookup_id, request_lookup_id_kind).
DROP INDEX IF EXISTS unique_pending_paid_lookup_id;

CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_paid_lookup_id
ON melt_quote(request_lookup_id, request_lookup_id_kind)
WHERE state IN ('PENDING', 'PAID');
