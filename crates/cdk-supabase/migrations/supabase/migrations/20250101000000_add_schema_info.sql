-- Migration 004: Add schema_info table for client-side compatibility checks
-- This table allows authenticated clients to verify the schema version
-- without needing access to the migrations table or exec_sql RPC.

-- ============================================================================
-- SCHEMA INFO TABLE
-- ============================================================================

CREATE TABLE IF NOT EXISTS schema_info (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Insert the current schema version (updated by each migration)
INSERT INTO schema_info (key, value) VALUES ('schema_version', '4')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;

-- Enable RLS and allow all authenticated users to read (but not write)
ALTER TABLE schema_info ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Anyone can read schema_info" ON schema_info
    FOR SELECT USING (true);

GRANT SELECT ON schema_info TO authenticated;
GRANT ALL ON schema_info TO service_role;
