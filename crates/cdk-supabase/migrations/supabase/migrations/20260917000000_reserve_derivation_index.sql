-- Catch-up and reservation must be atomic across clients using the same wallet.
CREATE OR REPLACE FUNCTION public.reserve_derivation_index(
    p_namespace TEXT,
    p_minimum_index BIGINT
)
RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = ''
AS $body$
DECLARE
    next_counter BIGINT;
BEGIN
    IF p_minimum_index IS NULL OR p_minimum_index < 0 OR p_minimum_index >= 4294967295 THEN
        RAISE EXCEPTION 'Derivation counter exhausted or invalid minimum' USING ERRCODE = '22003';
    END IF;
    INSERT INTO public.derivation_counter (wallet_id, namespace, counter)
    VALUES (public.get_current_wallet_id(), p_namespace, p_minimum_index + 1)
    ON CONFLICT (wallet_id, namespace) DO UPDATE
    SET counter = GREATEST(derivation_counter.counter, p_minimum_index) + 1
    WHERE derivation_counter.counter < 4294967295
    RETURNING counter INTO next_counter;
    IF next_counter IS NULL THEN
        RAISE EXCEPTION 'Derivation counter exhausted' USING ERRCODE = '22003';
    END IF;
    RETURN next_counter - 1;
END
$body$;

REVOKE ALL ON FUNCTION public.reserve_derivation_index(TEXT, BIGINT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.reserve_derivation_index(TEXT, BIGINT) TO authenticated, service_role;

INSERT INTO schema_info (key, value) VALUES ('schema_version', '11')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
