-- These are u64 in Rust. int4 wrapped anything above 2^31 into a negative row
-- that the checked read path then rejected, failing every keyset read.
ALTER TABLE keyset ALTER COLUMN valid_from TYPE TEXT USING valid_from::TEXT;
ALTER TABLE keyset ALTER COLUMN valid_to TYPE TEXT USING valid_to::TEXT;
ALTER TABLE keyset ALTER COLUMN input_fee_ppk TYPE TEXT USING input_fee_ppk::TEXT;
