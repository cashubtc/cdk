CREATE FUNCTION public.restore_proofs(
    p_proofs JSONB,
    p_persisted_ys JSONB,
    p_receive_sagas JSONB,
    p_saga_versions JSONB,
    p_keyset_id TEXT,
    p_keyset_counter BIGINT
)
RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY INVOKER
SET search_path = ''
AS $body$
DECLARE
    v_wallet TEXT := public.get_current_wallet_id();
    v_saga RECORD;
    v_counter BIGINT;
BEGIN
    FOR v_saga IN SELECT key, value::BIGINT AS version
        FROM pg_catalog.jsonb_each_text(p_saga_versions) ORDER BY key
    LOOP
        PERFORM 1 FROM public.saga
            WHERE wallet_id = v_wallet AND id = v_saga.key AND version = v_saga.version
              AND data::JSONB->'state'->>'type' = 'receive'
            FOR UPDATE;
        IF NOT FOUND THEN RETURN FALSE; END IF;
    END LOOP;
    FOR v_saga IN SELECT key, value::BIGINT AS version
        FROM pg_catalog.jsonb_each_text(p_receive_sagas) ORDER BY key
    LOOP
        PERFORM 1 FROM public.saga
            WHERE wallet_id = v_wallet AND id = v_saga.key AND version = v_saga.version
              AND data::JSONB->'state'->>'type' = 'receive'
              AND data::JSONB->'state'->>'state' = 'swap_requested'
              AND data::JSONB->'data'->>'kind' = 'receive'
            FOR UPDATE;
        IF NOT FOUND THEN RETURN FALSE; END IF;
    END LOOP;

    INSERT INTO public.keyset_counter AS existing (keyset_id, wallet_id, counter)
        SELECT p_keyset_id, v_wallet, 0
        WHERE p_keyset_counter = 0 OR EXISTS (
            SELECT 1 FROM public.keyset_counter
            WHERE wallet_id = v_wallet AND keyset_id = p_keyset_id AND counter = p_keyset_counter)
        ON CONFLICT (keyset_id, wallet_id) DO UPDATE SET counter = existing.counter
        WHERE existing.counter = p_keyset_counter
        RETURNING counter INTO v_counter;
    IF NOT FOUND THEN RETURN FALSE; END IF;

    UPDATE public.saga SET
        version = version + 1,
        data = pg_catalog.jsonb_set(
            pg_catalog.jsonb_set(data::JSONB, '{data,data,atomic_settlement}', 'false'::JSONB),
            '{version}', pg_catalog.to_jsonb(version + 1))::TEXT
        WHERE wallet_id = v_wallet AND id IN (
            SELECT key FROM pg_catalog.jsonb_each_text(p_receive_sagas));

    INSERT INTO public.keyset_counter AS existing (keyset_id, wallet_id, counter)
        SELECT p->>'keyset_id', v_wallet, max((p->>'derivation_index')::INTEGER) + 1
        FROM pg_catalog.jsonb_array_elements(p_proofs) p
        WHERE p->>'derivation_index' IS NOT NULL
        GROUP BY p->>'keyset_id' ORDER BY p->>'keyset_id'
        ON CONFLICT (keyset_id, wallet_id) DO UPDATE
        SET counter = greatest(existing.counter, EXCLUDED.counter);

    UPDATE public.proof AS existing SET state = 'PENDING'
        FROM pg_catalog.jsonb_array_elements(p_proofs) p
        WHERE existing.wallet_id = v_wallet AND existing.y = p->>'y'
          AND existing.state = 'UNSPENT' AND p->>'state' = 'PENDING';

    DELETE FROM public.proof WHERE wallet_id = v_wallet AND y IN (
        SELECT p->>'y' FROM pg_catalog.jsonb_array_elements(p_proofs) p
        WHERE p->>'state' = 'SPENT');

    INSERT INTO public.proof (
        y, wallet_id, mint_url, state, spending_condition, unit, amount,
        keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r,
        used_by_operation, created_by_operation, p2pk_e, derivation_index
    )
    SELECT p->>'y', v_wallet, p->>'mint_url', p->>'state', p->>'spending_condition',
        p->>'unit', (p->>'amount')::BIGINT, p->>'keyset_id', p->>'secret', p->>'c',
        p->>'witness', p->>'dleq_e', p->>'dleq_s', p->>'dleq_r',
        p->>'used_by_operation', p->>'created_by_operation', p->>'p2pk_e',
        (p->>'derivation_index')::INTEGER
    FROM pg_catalog.jsonb_array_elements(p_proofs) p
    WHERE p->>'state' != 'SPENT'
      AND NOT (p->>'y' = ANY(SELECT pg_catalog.jsonb_array_elements_text(p_persisted_ys)))
    ON CONFLICT (y, wallet_id) DO NOTHING;
    RETURN TRUE;
END;
$body$;
