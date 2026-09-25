-- blind_signature.amount is a u64 amount with no cap and the keyset validity
-- bounds are u64 timestamps, so int4 could not hold the range the write path
-- accepts. The keyset's derivation_path_index is a local rotation counter and
-- stays int4.
ALTER TABLE blind_signature ALTER COLUMN amount TYPE BIGINT;

ALTER TABLE keyset
    ALTER COLUMN valid_from TYPE BIGINT,
    ALTER COLUMN valid_to TYPE BIGINT;
