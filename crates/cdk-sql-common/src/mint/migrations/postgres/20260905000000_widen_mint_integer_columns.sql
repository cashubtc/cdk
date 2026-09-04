-- Every value bound from Rust travels as an i64, so int4 could not represent
-- the range the write path accepts. The driver narrowed out of range values
-- instead of refusing them, committing negative rows that the checked read path
-- could not decode, which failed every read of the table rather than only the
-- bad row. Widening every remaining column keeps the whole i64 range lossless.

ALTER TABLE keyset ALTER COLUMN derivation_path_index TYPE BIGINT;

ALTER TABLE melt_quote
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN fee_reserve TYPE BIGINT,
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN created_time TYPE BIGINT,
    ALTER COLUMN paid_time TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT,
    ALTER COLUMN selected_fee_index TYPE BIGINT;

ALTER TABLE melt_request
    ALTER COLUMN inputs_amount TYPE BIGINT,
    ALTER COLUMN inputs_fee TYPE BIGINT;

ALTER TABLE proof
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN created_time TYPE BIGINT;

ALTER TABLE blind_signature
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN created_time TYPE BIGINT,
    ALTER COLUMN signed_time TYPE BIGINT,
    ALTER COLUMN order_index TYPE BIGINT;

ALTER TABLE mint_quote
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN created_time TYPE BIGINT,
    ALTER COLUMN amount_paid TYPE BIGINT,
    ALTER COLUMN amount_issued TYPE BIGINT;

ALTER TABLE mint_quote_payments
    ALTER COLUMN id TYPE BIGINT,
    ALTER COLUMN timestamp TYPE BIGINT,
    ALTER COLUMN amount TYPE BIGINT;
ALTER SEQUENCE mint_quote_payments_id_seq AS BIGINT;

ALTER TABLE mint_quote_issued
    ALTER COLUMN id TYPE BIGINT,
    ALTER COLUMN amount TYPE BIGINT,
    ALTER COLUMN timestamp TYPE BIGINT;
ALTER SEQUENCE mint_quote_issued_id_seq AS BIGINT;

ALTER TABLE keyset_epoch ALTER COLUMN id TYPE BIGINT;
