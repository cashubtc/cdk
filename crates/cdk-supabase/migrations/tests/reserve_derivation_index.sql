-- Run only in the disposable database created by misc/pgbouncer/test-derivation-rpc.sh.
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
CREATE TABLE public.derivation_counter (
    wallet_id TEXT NOT NULL,
    namespace TEXT NOT NULL,
    counter BIGINT NOT NULL CHECK (counter >= 0),
    PRIMARY KEY (wallet_id, namespace)
);
-- Match Supabase's request identity without requiring its HTTP/auth service.
CREATE FUNCTION public.get_current_wallet_id() RETURNS TEXT LANGUAGE sql STABLE
AS $body$ SELECT current_setting('request.jwt.claims')::json->>'sub' $body$;
\ir ../supabase/migrations/20260917000000_reserve_derivation_index.sql

SET LOCAL ROLE authenticated;
SET LOCAL request.jwt.claims = '{"sub":"wallet-a"}';
DO $body$
BEGIN
    IF public.reserve_derivation_index('p2pk', 6) <> 6 OR
       public.reserve_derivation_index('p2pk', 6) <> 7 OR
       public.reserve_derivation_index('p2pk', 30) <> 30 OR
       public.reserve_derivation_index('p2pk', 0) <> 31 OR
       public.reserve_derivation_index('other', 0) <> 0 THEN
        RAISE EXCEPTION 'Reservation did not advance correctly';
    END IF;
    IF public.reserve_derivation_index('limit', 4294967294) <> 4294967294 THEN
        RAISE EXCEPTION 'Last reservation failed';
    END IF;
    BEGIN
        PERFORM public.reserve_derivation_index('limit', 0);
        RAISE EXCEPTION 'Exhausted counter was accepted';
    EXCEPTION WHEN numeric_value_out_of_range THEN NULL;
    END;
    BEGIN
        PERFORM public.reserve_derivation_index('invalid', 4294967295);
        RAISE EXCEPTION 'Overflowing minimum was accepted';
    EXCEPTION WHEN numeric_value_out_of_range THEN NULL;
    END;
END
$body$;
SET LOCAL request.jwt.claims = '{"sub":"wallet-b"}';
DO $body$
BEGIN
    IF public.reserve_derivation_index('p2pk', 0) <> 0 THEN
        RAISE EXCEPTION 'Wallet identities are not isolated';
    END IF;
END
$body$;
RESET ROLE;
DO $body$
BEGIN
    IF (SELECT counter FROM public.derivation_counter WHERE wallet_id='wallet-a' AND namespace='limit') <> 4294967295 OR
       EXISTS (SELECT FROM public.derivation_counter WHERE namespace='invalid') THEN
        RAISE EXCEPTION 'Overflow changed persisted data';
    END IF;
END
$body$;
ROLLBACK;
