-- See the mint migration of the same shape. Every value bound from Rust travels
-- as an i64, so int4 could not represent the range the write path accepts.
ALTER TABLE keyset ALTER COLUMN derivation_path_index TYPE BIGINT;
ALTER TABLE blind_signature ALTER COLUMN amount TYPE BIGINT;
