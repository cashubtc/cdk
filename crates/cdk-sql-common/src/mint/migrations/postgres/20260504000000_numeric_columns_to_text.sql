-- Migrate amount/fee columns to TEXT
-- u64 amounts cannot be faithfully represented as PostgreSQL INTEGER/BIGINT (signed i64)
-- Only amount and fee columns are migrated; timestamps and indices stay as INTEGER/BIGINT

-- proof
ALTER TABLE proof ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE proof ADD CONSTRAINT chk_proof_amount_numeric CHECK (amount ~ '^\d+$');

-- blind_signature
ALTER TABLE blind_signature ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE blind_signature ADD CONSTRAINT chk_blind_signature_amount_numeric CHECK (amount ~ '^\d+$');

-- mint_quote
ALTER TABLE mint_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_paid TYPE TEXT USING amount_paid::TEXT;
ALTER TABLE mint_quote ALTER COLUMN amount_issued TYPE TEXT USING amount_issued::TEXT;
ALTER TABLE mint_quote ADD CONSTRAINT chk_mint_quote_amount_numeric CHECK (amount ~ '^\d+$');
ALTER TABLE mint_quote ADD CONSTRAINT chk_mint_quote_amount_paid_numeric CHECK (amount_paid ~ '^\d+$');
ALTER TABLE mint_quote ADD CONSTRAINT chk_mint_quote_amount_issued_numeric CHECK (amount_issued ~ '^\d+$');

-- mint_quote_payments
ALTER TABLE mint_quote_payments ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE mint_quote_payments ADD CONSTRAINT chk_mint_quote_payments_amount_numeric CHECK (amount ~ '^\d+$');

-- mint_quote_issued
ALTER TABLE mint_quote_issued ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE mint_quote_issued ADD CONSTRAINT chk_mint_quote_issued_amount_numeric CHECK (amount ~ '^\d+$');

-- melt_quote
ALTER TABLE melt_quote ALTER COLUMN amount TYPE TEXT USING amount::TEXT;
ALTER TABLE melt_quote ALTER COLUMN fee_reserve TYPE TEXT USING fee_reserve::TEXT;
ALTER TABLE melt_quote ADD CONSTRAINT chk_melt_quote_amount_numeric CHECK (amount ~ '^\d+$');
ALTER TABLE melt_quote ADD CONSTRAINT chk_melt_quote_fee_reserve_numeric CHECK (fee_reserve ~ '^\d+$');

-- melt_request
ALTER TABLE melt_request ALTER COLUMN inputs_amount TYPE TEXT USING inputs_amount::TEXT;
ALTER TABLE melt_request ALTER COLUMN inputs_fee TYPE TEXT USING inputs_fee::TEXT;
ALTER TABLE melt_request ADD CONSTRAINT chk_melt_request_inputs_amount_numeric CHECK (inputs_amount ~ '^\d+$');
ALTER TABLE melt_request ADD CONSTRAINT chk_melt_request_inputs_fee_numeric CHECK (inputs_fee ~ '^\d+$');

-- keyset_amounts
ALTER TABLE keyset_amounts ALTER COLUMN total_issued TYPE TEXT USING total_issued::TEXT;
ALTER TABLE keyset_amounts ALTER COLUMN total_redeemed TYPE TEXT USING total_redeemed::TEXT;
ALTER TABLE keyset_amounts ALTER COLUMN fee_collected TYPE TEXT USING fee_collected::TEXT;
ALTER TABLE keyset_amounts ADD CONSTRAINT chk_keyset_amounts_total_issued_numeric CHECK (total_issued ~ '^\d+$');
ALTER TABLE keyset_amounts ADD CONSTRAINT chk_keyset_amounts_total_redeemed_numeric CHECK (total_redeemed ~ '^\d+$');
ALTER TABLE keyset_amounts ADD CONSTRAINT chk_keyset_amounts_fee_collected_numeric CHECK (fee_collected ~ '^\d+$');

-- completed_operations
ALTER TABLE completed_operations ALTER COLUMN total_issued TYPE TEXT USING total_issued::TEXT;
ALTER TABLE completed_operations ALTER COLUMN total_redeemed TYPE TEXT USING total_redeemed::TEXT;
ALTER TABLE completed_operations ALTER COLUMN fee_collected TYPE TEXT USING fee_collected::TEXT;
ALTER TABLE completed_operations ALTER COLUMN payment_amount TYPE TEXT USING payment_amount::TEXT;
ALTER TABLE completed_operations ALTER COLUMN payment_fee TYPE TEXT USING payment_fee::TEXT;
ALTER TABLE completed_operations ADD CONSTRAINT chk_completed_ops_total_issued_numeric CHECK (total_issued ~ '^\d+$');
ALTER TABLE completed_operations ADD CONSTRAINT chk_completed_ops_total_redeemed_numeric CHECK (total_redeemed ~ '^\d+$');
ALTER TABLE completed_operations ADD CONSTRAINT chk_completed_ops_fee_collected_numeric CHECK (fee_collected ~ '^\d+$');
ALTER TABLE completed_operations ADD CONSTRAINT chk_completed_ops_payment_amount_numeric CHECK (payment_amount ~ '^\d+$');
ALTER TABLE completed_operations ADD CONSTRAINT chk_completed_ops_payment_fee_numeric CHECK (payment_fee ~ '^\d+$');
