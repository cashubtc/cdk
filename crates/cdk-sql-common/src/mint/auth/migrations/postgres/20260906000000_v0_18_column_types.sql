-- The auth database gets the same treatment as the mint's own, on the two
-- tables that have a column the mint does not control.
--
-- blind_signature.amount is a u64 on the wire, so it becomes numeric(20, 0),
-- which holds the whole range. keyset's valid_from and valid_to are u64 in
-- Rust but bounded by a clock, so int8 is enough and their ordering stays
-- native. int4 wrapped anything above 2^31 into a negative row that the checked
-- read path then rejected, failing every read of the table.
--
-- keyset.derivation_path_index is a local rotation counter and stays int4.
--
-- u64, u64_add and u64_sum are declared here too. This database runs no amount
-- arithmetic today, but the three names exist on both backends so that a
-- statement touching an amount is written once and runs on either, without
-- first asking which database it is against.
--
-- Each ALTER rewrites the table under an ACCESS EXCLUSIVE lock.

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
-- rows total to zero rather than NULL, which is what every caller wants and
-- what the SQLite implementation returns. The combine function is what keeps a
-- large scan parallelizable.
DROP AGGREGATE IF EXISTS u64_sum(NUMERIC);
CREATE AGGREGATE u64_sum(NUMERIC) (
    SFUNC = numeric_add,
    STYPE = NUMERIC,
    COMBINEFUNC = numeric_add,
    INITCOND = '0',
    PARALLEL = SAFE
);

ALTER TABLE keyset
    ALTER COLUMN valid_from TYPE BIGINT,
    ALTER COLUMN valid_to TYPE BIGINT;

ALTER TABLE blind_signature
    ALTER COLUMN amount TYPE NUMERIC(20, 0);
