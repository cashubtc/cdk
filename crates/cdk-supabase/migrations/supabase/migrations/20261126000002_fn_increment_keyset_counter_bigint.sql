CREATE OR REPLACE FUNCTION public.increment_keyset_counter(
    p_keyset_id TEXT,
    p_increment BIGINT DEFAULT 1
)
RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = ''
AS $body$
DECLARE
    new_counter BIGINT;
BEGIN
    IF p_increment IS NULL OR p_increment < 0 OR p_increment > 4294967295 THEN
        RAISE EXCEPTION 'Invalid keyset counter increment' USING ERRCODE = '22003';
    END IF;
    INSERT INTO public.keyset_counter (keyset_id, wallet_id, counter)
    VALUES (p_keyset_id, public.get_current_wallet_id(), p_increment)
    ON CONFLICT (keyset_id, wallet_id) DO UPDATE
    SET counter = keyset_counter.counter + p_increment
    WHERE keyset_counter.counter <= 4294967295 - p_increment
    RETURNING counter INTO new_counter;
    IF new_counter IS NULL THEN
        RAISE EXCEPTION 'Keyset counter overflow' USING ERRCODE = '22003';
    END IF;
    RETURN new_counter;
END
$body$;
