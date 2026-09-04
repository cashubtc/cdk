-- Widen only the columns whose value can outgrow a signed 32-bit one: u64
-- amounts, unix timestamps, and the fields the mint controls. mint_time,
-- input_fee_ppk and final_expiry are u64 straight off the wire, and
-- estimated_blocks and fee_index are u32 the wallet stores without a range
-- check, so a peer rather than a clock decides how large they get.
--
-- The integer columns left out of this file are produced locally and bounded by
-- construction: the version columns are optimistic-lock counters, the
-- derivation indexes and keyset_counter are local BIP32 and NUT-13 counters,
-- and keyset_u32 is `int % (2^31 - 1)` so it cannot exceed a signed 32-bit
-- column. The driver refuses a value an int4 column cannot hold instead of
-- truncating it, so those stay as they are.
--
-- Each ALTER rewrites the table under an ACCESS EXCLUSIVE lock; the
-- operational cost is measured in docs/migrations/v0.18.md.

ALTER TABLE mint ALTER COLUMN mint_time TYPE BIGINT;

ALTER TABLE keyset
    ALTER COLUMN input_fee_ppk TYPE BIGINT,
    ALTER COLUMN final_expiry TYPE BIGINT;

ALTER TABLE melt_quote
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN fee_reserve TYPE BIGINT,
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT,
    ALTER COLUMN fee_index TYPE BIGINT;

ALTER TABLE proof ALTER COLUMN amount TYPE BIGINT;

ALTER TABLE transactions
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN fee TYPE BIGINT,
    ALTER COLUMN timestamp TYPE BIGINT;

ALTER TABLE mint_quote
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN amount_paid TYPE BIGINT,
    ALTER COLUMN amount_issued TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT;
