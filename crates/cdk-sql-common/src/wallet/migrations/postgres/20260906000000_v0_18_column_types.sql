-- Every column holding a value the mint sends the wallet gets a type wide
-- enough for it.
--
-- Amounts are u64 on the wire, and int8 covers all but the top half of that
-- range. They become numeric(20, 0), which holds every one of them.
--
-- Everything else that had outgrown int4 becomes int8. mint_time,
-- input_fee_ppk and final_expiry are u64 straight off the wire, and
-- estimated_blocks and fee_index are u32 the wallet stores without a range
-- check, so a peer rather than a clock decides how large they get. These stay
-- integers so their ordering and arithmetic remain native.
--
-- The integer columns left out of this file are produced locally and bounded by
-- construction: the version columns are optimistic-lock counters, the
-- derivation indexes and keyset_counter are local BIP32 and NUT-13 counters,
-- and keyset_u32 is `int % (2^31 - 1)` so it cannot exceed a signed 32-bit
-- column. The driver refuses a value an int4 column cannot hold instead of
-- truncating it, so those stay as they are.
--
-- u64_sum exists so the balance query works on both backends. Here it is an
-- alias of arithmetic postgres already does exactly; SQLite has no unsigned
-- type and promotes an integer overflow to a float, so it implements the
-- aggregate in rust and this side matches its name.
--
-- Each ALTER rewrites the table under an ACCESS EXCLUSIVE lock, so every column
-- of a table changes in one statement and no table is rewritten twice.

-- u64 is the conversion into the representation an amount column holds. SQLite
-- stores one as a blob, so a value SQL names rather than binds has to be
-- encoded before it can be compared against a column; here the column already
-- holds a number and the conversion is the identity. It exists so a statement
-- comparing an amount is written once and runs on either backend.
CREATE OR REPLACE FUNCTION u64(a NUMERIC) RETURNS NUMERIC
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$ SELECT a $$;

CREATE OR REPLACE FUNCTION u64_add(a NUMERIC, b NUMERIC) RETURNS NUMERIC
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$ SELECT a + b $$;

-- numeric_add is the strict function behind the numeric + operator, so the
-- aggregate skips NULL rows the way SUM does. The initial condition makes no
-- rows total to zero rather than NULL, which is what the balance query wants
-- and what the SQLite implementation returns. The combine function is what
-- keeps a large scan parallelizable.
DROP AGGREGATE IF EXISTS u64_sum(NUMERIC);
CREATE AGGREGATE u64_sum(NUMERIC) (
    SFUNC = numeric_add,
    STYPE = NUMERIC,
    COMBINEFUNC = numeric_add,
    INITCOND = '0',
    PARALLEL = SAFE
);

ALTER TABLE mint ALTER COLUMN mint_time TYPE BIGINT;

ALTER TABLE keyset
    ALTER COLUMN input_fee_ppk TYPE BIGINT,
    ALTER COLUMN final_expiry TYPE BIGINT;

ALTER TABLE proof
    ALTER COLUMN amount TYPE NUMERIC(20, 0);

ALTER TABLE melt_quote
    ALTER COLUMN amount TYPE NUMERIC(20, 0),
    ALTER COLUMN fee_reserve TYPE NUMERIC(20, 0),
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT,
    ALTER COLUMN fee_index TYPE BIGINT;

ALTER TABLE mint_quote
    ALTER COLUMN amount TYPE NUMERIC(20, 0),
    ALTER COLUMN amount_paid TYPE NUMERIC(20, 0),
    ALTER COLUMN amount_issued TYPE NUMERIC(20, 0),
    ALTER COLUMN expiry TYPE BIGINT,
    ALTER COLUMN estimated_blocks TYPE BIGINT;

ALTER TABLE transactions
    ALTER COLUMN amount TYPE NUMERIC(20, 0),
    ALTER COLUMN fee TYPE NUMERIC(20, 0),
    ALTER COLUMN timestamp TYPE BIGINT;

ALTER TABLE wallet_sagas
    ALTER COLUMN amount TYPE NUMERIC(20, 0);
