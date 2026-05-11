-- Migrate amount/fee BIGINT columns to TEXT
-- u64 amounts cannot be faithfully represented as PostgreSQL BIGINT (signed i64)
-- Only amount and fee columns are migrated; timestamps and indices stay as BIGINT

-- mint_quote
ALTER TABLE mint_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_issued TYPE TEXT USING amount_issued::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_paid TYPE TEXT USING amount_paid::TEXT;
ALTER TABLE mint_quote ADD CONSTRAINT chk_mint_quote_amount_numeric CHECK (amount ~ '^\d+$');
ALTER TABLE mint_quote ADD CONSTRAINT chk_mint_quote_amount_issued_numeric CHECK (amount_issued ~ '^\d+$');
ALTER TABLE mint_quote ADD CONSTRAINT chk_mint_quote_amount_paid_numeric CHECK (amount_paid ~ '^\d+$');

-- melt_quote
ALTER TABLE melt_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE melt_quote ALTER COLUMN fee_reserve TYPE TEXT USING fee_reserve::TEXT;
ALTER TABLE melt_quote ADD CONSTRAINT chk_melt_quote_amount_numeric CHECK (amount ~ '^\d+$');
ALTER TABLE melt_quote ADD CONSTRAINT chk_melt_quote_fee_reserve_numeric CHECK (fee_reserve ~ '^\d+$');

-- proof
ALTER TABLE proof ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE proof ADD CONSTRAINT chk_proof_amount_numeric CHECK (amount ~ '^\d+$');

-- transactions
ALTER TABLE transactions ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE transactions ALTER COLUMN fee TYPE TEXT USING fee::TEXT;
ALTER TABLE transactions ADD CONSTRAINT chk_transactions_amount_numeric CHECK (amount ~ '^\d+$');
ALTER TABLE transactions ADD CONSTRAINT chk_transactions_fee_numeric CHECK (fee ~ '^\d+$');
