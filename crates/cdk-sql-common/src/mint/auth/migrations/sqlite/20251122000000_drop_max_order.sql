-- Drop max_order column from keyset table
-- SQLite doesn't support DROP COLUMN directly, so we need to recreate the table

-- Create new table without max_order
CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    valid_from INTEGER NOT NULL,
    valid_to INTEGER,
    derivation_path TEXT NOT NULL,
    derivation_path_index INTEGER NOT NULL
);

-- Copy data from old table to new table
INSERT INTO keyset_new (id, unit, active, valid_from, valid_to, derivation_path, derivation_path_index)
SELECT id, unit, active, valid_from, valid_to, derivation_path, derivation_path_index
FROM keyset;

-- Drop old table
DROP TABLE keyset;

-- Rename new table to original name
ALTER TABLE keyset_new RENAME TO keyset;

-- Recreate indexes
CREATE INDEX IF NOT EXISTS unit_index ON keyset(unit);
CREATE INDEX IF NOT EXISTS active_index ON keyset(active);
