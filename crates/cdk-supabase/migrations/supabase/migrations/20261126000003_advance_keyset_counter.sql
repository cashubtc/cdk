-- Advancing a keyset counter during restore must be atomic across clients.
CREATE OR REPLACE FUNCTION public.advance_keyset_counter(
    p_keyset_id TEXT,
    p_minimum_counter BIGINT
)
RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = ''
AS $body$
DECLARE
    new_counter BIGINT;
BEGIN
    IF p_minimum_counter IS NULL OR p_minimum_counter < 0 OR p_minimum_counter > 4294967295 THEN
        RAISE EXCEPTION 'Invalid keyset counter minimum' USING ERRCODE = '22003';
    END IF;
    INSERT INTO public.keyset_counter (keyset_id, wallet_id, counter)
    VALUES (p_keyset_id, public.get_current_wallet_id(), p_minimum_counter)
    ON CONFLICT (keyset_id, wallet_id) DO UPDATE
    SET counter = GREATEST(keyset_counter.counter, p_minimum_counter)
    RETURNING counter INTO new_counter;
    RETURN new_counter;
END
$body$;
