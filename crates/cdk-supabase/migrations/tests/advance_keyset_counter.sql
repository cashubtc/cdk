-- Run only against an empty, disposable PostgreSQL database, for example:
--   createdb cdk_rpc_test
--   psql --no-psqlrc --dbname cdk_rpc_test \
--     --file crates/cdk-supabase/migrations/tests/advance_keyset_counter.sql
--   dropdb cdk_rpc_test
-- Everything runs in one transaction that is rolled back.
\set ON_ERROR_STOP on
BEGIN;
DO $body$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'authenticated') THEN
        CREATE ROLE authenticated;
    END IF;
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'service_role') THEN
        CREATE ROLE service_role;
    END IF;
END
$body$;
CREATE TABLE public.schema_info (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE public.keyset_counter (
    keyset_id TEXT NOT NULL,
    wallet_id TEXT NOT NULL,
    counter INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (keyset_id, wallet_id)
);
CREATE FUNCTION public.get_current_wallet_id() RETURNS TEXT LANGUAGE sql STABLE
AS $body$ SELECT current_setting('request.jwt.claims')::json->>'sub' $body$;
\ir ../supabase/migrations/20241001000002_fn_increment_keyset_counter.sql
\ir ../supabase/migrations/20261126000000_keyset_counter_bigint.sql
\ir ../supabase/migrations/20261126000001_drop_increment_keyset_counter_int.sql
\ir ../supabase/migrations/20261126000002_fn_increment_keyset_counter_bigint.sql
\ir ../supabase/migrations/20261126000003_advance_keyset_counter.sql
\ir ../supabase/migrations/20261126000004_keyset_counter_grants.sql
\ir ../supabase/migrations/20261126000005_schema_version_12.sql

SET LOCAL ROLE authenticated;
SET LOCAL request.jwt.claims = '{"sub":"wallet-a"}';
DO $body$
BEGIN
    IF public.advance_keyset_counter('009a1f293253e41e', 5) <> 5 OR
       public.advance_keyset_counter('009a1f293253e41e', 3) <> 5 OR
       public.advance_keyset_counter('009a1f293253e41e', 20) <> 20 OR
       public.increment_keyset_counter('009a1f293253e41e', 10) <> 30 OR
       public.advance_keyset_counter('009a1f293253e41e', 25) <> 30 OR
       public.advance_keyset_counter('00ad268c4d1f5826', 4294967295) <> 4294967295 OR
       public.increment_keyset_counter('00ad268c4d1f5826', 0) <> 4294967295 THEN
        RAISE EXCEPTION 'Keyset counter did not advance monotonically';
    END IF;
    BEGIN
        PERFORM public.increment_keyset_counter('00ad268c4d1f5826', 1);
        RAISE EXCEPTION 'Overflowing keyset counter increment was accepted';
    EXCEPTION WHEN numeric_value_out_of_range THEN NULL;
    END;
    BEGIN
        PERFORM public.advance_keyset_counter('009a1f293253e41e', 4294967296);
        RAISE EXCEPTION 'Overflowing keyset counter minimum was accepted';
    EXCEPTION WHEN numeric_value_out_of_range THEN NULL;
    END;
END
$body$;
SET LOCAL request.jwt.claims = '{"sub":"wallet-b"}';
DO $body$
BEGIN
    IF public.advance_keyset_counter('009a1f293253e41e', 0) <> 0 THEN
        RAISE EXCEPTION 'Wallet identities are not isolated';
    END IF;
END
$body$;
RESET ROLE;
DO $body$
BEGIN
    IF (SELECT counter FROM public.keyset_counter WHERE wallet_id='wallet-a' AND keyset_id='00ad268c4d1f5826') <> 4294967295 THEN
        RAISE EXCEPTION 'Overflow changed persisted data';
    END IF;
    IF (SELECT value FROM public.schema_info WHERE key='schema_version') IS DISTINCT FROM '12' THEN
        RAISE EXCEPTION 'Schema version was not bumped';
    END IF;
END
$body$;
ROLLBACK;
