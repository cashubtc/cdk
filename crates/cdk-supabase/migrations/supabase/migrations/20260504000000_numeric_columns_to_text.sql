-- Migrate amount/fee BIGINT columns to TEXT
-- u64 amounts cannot be faithfully represented as PostgreSQL BIGINT (signed i64)
-- Only amount and fee columns are migrated; timestamps and indices stay as BIGINT

-- mint_quote
ALTER TABLE mint_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_issued TYPE TEXT USING amount_issued::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_paid TYPE TEXT USING amount_paid::TEXT;

-- melt_quote
ALTER TABLE melt_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE melt_quote ALTER COLUMN fee_reserve TYPE TEXT USING fee_reserve::TEXT;

-- proof
ALTER TABLE proof ALTER COLUMN amount TYPE TEXT USING amount::TEXT;

-- transactions
ALTER TABLE transactions ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE transactions ALTER COLUMN fee TYPE TEXT USING fee::TEXT;
