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

-- Drop the counter column from keyset table (SQLite requires table recreation)
-- Step 1: Create new keyset table without counter column
CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    keyset_u32 INTEGER,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER,
    final_expiry INTEGER DEFAULT NULL,
    FOREIGN KEY(mint_url) REFERENCES mint(mint_url) ON UPDATE CASCADE ON DELETE CASCADE
);

-- Step 2: Copy data from old keyset table (excluding counter)
INSERT INTO keyset_new (id, keyset_u32, mint_url, unit, active, input_fee_ppk, final_expiry)
SELECT id, keyset_u32, mint_url, unit, active, input_fee_ppk, final_expiry
FROM keyset;

-- Step 3: Drop old keyset table
DROP TABLE keyset;

-- Step 4: Rename new table to keyset
ALTER TABLE keyset_new RENAME TO keyset;
