-- See the mint migration of the same shape. Every value bound from Rust travels
-- as an i64, so int4 could not represent the range the write path accepts. The
-- mint controls input_fee_ppk and final_expiry, so those two were reachable by
-- a remote peer rather than only by a clock.

ALTER TABLE mint ALTER COLUMN mint_time TYPE BIGINT;

ALTER TABLE keyset
    ALTER COLUMN input_fee_ppk TYPE BIGINT,
    ALTER COLUMN final_expiry TYPE BIGINT,
    ALTER COLUMN keyset_u32 TYPE BIGINT;

ALTER TABLE key ALTER COLUMN keyset_u32 TYPE BIGINT;

ALTER TABLE melt_quote
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN fee_reserve TYPE BIGINT,
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN version TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT,
    ALTER COLUMN fee_index TYPE BIGINT;

ALTER TABLE proof
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN derivation_index TYPE BIGINT;

ALTER TABLE transactions
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN fee TYPE BIGINT,
    ALTER COLUMN timestamp TYPE BIGINT;

ALTER TABLE mint_quote
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN amount_paid TYPE BIGINT,
    ALTER COLUMN amount_issued TYPE BIGINT,
    ALTER COLUMN version TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT;

ALTER TABLE keyset_counter ALTER COLUMN counter TYPE BIGINT;
ALTER TABLE p2pk_signing_key ALTER COLUMN derivation_index TYPE BIGINT;
ALTER TABLE wallet_sagas ALTER COLUMN version TYPE BIGINT;
