-- Create dedicated keyset_counter table without foreign keys
-- This table tracks the counter for each keyset independently
CREATE TABLE IF NOT EXISTS keyset_counter (
    keyset_id TEXT PRIMARY KEY,
    counter INTEGER NOT NULL DEFAULT 0
);

-- Migrate existing counter values from keyset table
INSERT INTO keyset_counter (keyset_id, counter)
SELECT id, counter
FROM keyset
WHERE counter > 0;

-- Drop the counter column from keyset table
ALTER TABLE keyset DROP COLUMN counter;
