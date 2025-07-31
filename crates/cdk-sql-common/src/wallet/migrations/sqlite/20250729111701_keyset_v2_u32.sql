-- Add u32 representation column to key table with unique constraint
ALTER TABLE key ADD COLUMN keyset_u32 INTEGER;

-- Add unique constraint on the new column
CREATE UNIQUE INDEX IF NOT EXISTS keyset_u32_unique ON key(keyset_u32);

-- Add u32 representation column to keyset table with unique constraint
ALTER TABLE keyset ADD COLUMN keyset_u32 INTEGER;

-- Add unique constraint on the new column
CREATE UNIQUE INDEX IF NOT EXISTS keyset_u32_unique_keyset ON keyset(keyset_u32);
