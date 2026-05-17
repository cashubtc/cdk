-- Composite index for paginated condition lookups filtered by attestation_status.
-- get_conditions issues `WHERE attestation_status IN (...) ORDER BY created_at LIMIT`,
-- which would otherwise scan the full conditions table.
CREATE INDEX IF NOT EXISTS idx_conditions_status_created
    ON conditions (attestation_status, created_at);
