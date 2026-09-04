-- See the mint migration of the same shape: valid_from and valid_to are u64 in
-- Rust, so int4 could not hold the range the write path accepts. The file name
-- differs from the mint one because migrations are keyed on the name alone.
ALTER TABLE keyset
    ALTER COLUMN valid_from TYPE BIGINT,
    ALTER COLUMN valid_to TYPE BIGINT;
