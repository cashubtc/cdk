-- These columns hold u64 values. int4 wrapped anything above 2^31 into a
-- negative row that the checked read path then rejected, failing every keyset
-- read. int8 covers the whole range the write path now accepts.
ALTER TABLE keyset
    ALTER COLUMN valid_from TYPE BIGINT,
    ALTER COLUMN valid_to TYPE BIGINT,
    ALTER COLUMN input_fee_ppk TYPE BIGINT;
