-- Migrate amount columns to TEXT
-- u64 amounts cannot be faithfully represented as PostgreSQL INTEGER (signed i64)

-- blind_signature
ALTER TABLE blind_signature ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE blind_signature ADD CONSTRAINT chk_blind_signature_amount_numeric CHECK (amount ~ '^\d+$');
