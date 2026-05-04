-- Migrate amount/fee columns to TEXT
-- u64 amounts cannot be faithfully represented as PostgreSQL INTEGER/BIGINT (signed i64)
-- Only amount and fee columns are migrated; timestamps and indices stay as INTEGER/BIGINT

-- proof
ALTER TABLE proof ALTER COLUMN amount TYPE TEXT USING amount::TEXT;

-- blind_signature
ALTER TABLE blind_signature ALTER COLUMN amount TYPE TEXT USING amount::TEXT;

-- mint_quote
ALTER TABLE mint_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_paid TYPE TEXT USING amount_paid::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_issued TYPE TEXT USING amount_issued::TEXT;

-- mint_quote_payments
ALTER TABLE mint_quote_payments ALTER COLUMN amount TYPE TEXT USING amount::TEXT;

-- mint_quote_issued
ALTER TABLE mint_quote_issued ALTER COLUMN amount TYPE TEXT USING amount::TEXT;

-- melt_quote
ALTER TABLE melt_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE melt_quote ALTER COLUMN fee_reserve TYPE TEXT USING fee_reserve::TEXT;

-- melt_request
ALTER TABLE melt_request ALTER COLUMN inputs_amount TYPE TEXT USING inputs_amount::TEXT;
ALTER TABLE melt_request ALTER COLUMN inputs_fee TYPE TEXT USING inputs_fee::TEXT;

-- keyset_amounts
ALTER TABLE keyset_amounts ALTER COLUMN total_issued TYPE TEXT USING total_issued::TEXT;
ALTER TABLE keyset_amounts ALTER COLUMN total_redeemed TYPE TEXT USING total_redeemed::TEXT;
ALTER TABLE keyset_amounts ALTER COLUMN fee_collected TYPE TEXT USING fee_collected::TEXT;

-- completed_operations
ALTER TABLE completed_operations ALTER COLUMN total_issued TYPE TEXT USING total_issued::TEXT;
ALTER TABLE completed_operations ALTER COLUMN total_redeemed TYPE TEXT USING total_redeemed::TEXT;
ALTER TABLE completed_operations ALTER COLUMN fee_collected TYPE TEXT USING fee_collected::TEXT;
ALTER TABLE completed_operations ALTER COLUMN payment_amount TYPE TEXT USING payment_amount::TEXT;
ALTER TABLE completed_operations ALTER COLUMN payment_fee TYPE TEXT USING payment_fee::TEXT;
