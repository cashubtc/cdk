-- blind_signature.amount is a u64 amount with no cap, so int4 could not hold
-- the range the write path accepts. The keyset's derivation_path_index is a
-- local rotation counter and stays int4.
ALTER TABLE blind_signature ALTER COLUMN amount TYPE BIGINT;
