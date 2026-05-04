-- Migrate amount columns to TEXT
-- u64 amounts cannot be faithfully represented as PostgreSQL INTEGER (signed i64)

-- blind_signature
ALTER TABLE blind_signature ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
