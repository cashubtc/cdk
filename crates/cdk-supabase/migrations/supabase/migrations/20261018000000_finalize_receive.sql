CREATE FUNCTION public.finalize_receive(
    p_saga_id TEXT,
    p_version BIGINT,
    p_outputs JSONB,
    p_persisted_ys JSONB
)
RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY INVOKER
SET search_path = ''
AS $body$
DECLARE
    v_wallet TEXT := public.get_current_wallet_id();
    v_transaction TEXT;
BEGIN
    IF pg_catalog.jsonb_array_length(p_outputs) = 0 THEN
        RAISE EXCEPTION 'Receive outputs are empty';
    END IF;

    PERFORM 1 FROM public.saga
        WHERE wallet_id = v_wallet AND id = p_saga_id AND version = p_version
          AND data::JSONB->'state'->>'type' = 'receive'
          AND data::JSONB->'state'->>'state' = 'swap_requested'
        FOR UPDATE;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    SELECT id INTO v_transaction FROM public.transactions
        WHERE wallet_id = v_wallet AND saga_id = p_saga_id
          AND id = pg_catalog.encode(pg_catalog.convert_to(pg_catalog.replace(p_saga_id, '-', ''), 'UTF8'), 'hex')
          AND direction = 'Incoming' AND status IN ('pending', 'completed')
          AND amount = (SELECT sum((p->>'amount')::BIGINT)
                        FROM pg_catalog.jsonb_array_elements(p_outputs) p)
        FOR UPDATE;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    INSERT INTO public.proof (
        y, wallet_id, mint_url, state, spending_condition, unit, amount,
        keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r,
        used_by_operation, created_by_operation, p2pk_e, derivation_index
    )
    SELECT
        p->>'y', v_wallet, p->>'mint_url', p->>'state', p->>'spending_condition',
        p->>'unit', (p->>'amount')::BIGINT, p->>'keyset_id', p->>'secret',
        p->>'c', p->>'witness', p->>'dleq_e', p->>'dleq_s', p->>'dleq_r',
        p->>'used_by_operation', p->>'created_by_operation', p->>'p2pk_e',
        (p->>'derivation_index')::INTEGER
    FROM pg_catalog.jsonb_array_elements(p_outputs) p
    WHERE NOT (p->>'y' = ANY(SELECT pg_catalog.jsonb_array_elements_text(p_persisted_ys)))
    ON CONFLICT (y, wallet_id) DO NOTHING;

    DELETE FROM public.proof WHERE wallet_id = v_wallet AND used_by_operation = p_saga_id;
    UPDATE public.transactions SET status = 'completed'
        WHERE wallet_id = v_wallet AND id = v_transaction AND status = 'pending';
    DELETE FROM public.saga WHERE wallet_id = v_wallet AND id = p_saga_id;
    RETURN TRUE;
END;
$body$;
