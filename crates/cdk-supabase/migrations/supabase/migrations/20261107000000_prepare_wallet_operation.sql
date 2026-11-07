-- Commit an initial wallet operation and its local resources together.
CREATE OR REPLACE FUNCTION public.prepare_wallet_operation(
    p_saga JSONB,
    p_proof_ys JSONB,
    p_proofs JSONB,
    p_quotes JSONB,
    p_counter JSONB
)
RETURNS VOID
LANGUAGE plpgsql
SECURITY INVOKER
SET search_path = ''
AS $body$
DECLARE
    wallet TEXT := public.get_current_wallet_id();
    operation TEXT := p_saga->>'id';
    mint TEXT := p_saga->>'mint_url';
    operation_unit TEXT := p_saga->>'unit';
    initial BOOLEAN := (p_saga->>'version')::INTEGER = 0;
    is_swap BOOLEAN := p_saga->>'kind' = 'swap';
    parent JSONB := p_saga#>'{data,data,parent}';
    is_mint BOOLEAN := p_saga->>'kind' = 'mint';
    q JSONB;
    p JSONB;
    input_y TEXT;
    existing public.proof%ROWTYPE;
    affected INTEGER;
    expected_quotes JSONB;
BEGIN
    IF wallet IS NULL OR operation IS NULL THEN
        RAISE EXCEPTION 'Invalid operation preparation';
    END IF;
    IF is_mint THEN
        expected_quotes := COALESCE(p_saga#>'{data,data,quote_ids}', pg_catalog.jsonb_build_array(p_saga#>>'{data,data,quote_id}'));
        IF NOT initial OR p_saga#>>'{state,type}' <> 'issue' OR p_saga#>>'{state,state}' <> 'secrets_prepared'
           OR p_saga#>>'{data,kind}' <> 'mint'
           OR pg_catalog.jsonb_array_length(p_proofs) + pg_catalog.jsonb_array_length(p_proof_ys) <> 0 THEN
            RAISE EXCEPTION 'Invalid mint preparation';
        END IF;
    ELSIF is_swap THEN
        expected_quotes := '[]'::JSONB;
        IF NOT initial OR p_saga#>>'{state,type}' <> 'swap' OR p_saga#>>'{state,state}' <> 'proofs_reserved'
           OR p_saga#>>'{data,kind}' <> 'swap' THEN RAISE EXCEPTION 'Invalid swap preparation'; END IF;
    ELSE
        expected_quotes := pg_catalog.jsonb_build_array(p_saga#>>'{data,data,quote_id}');
        IF p_saga->>'kind' <> 'melt' OR p_saga#>>'{state,type}' <> 'melt'
           OR (initial AND p_saga#>>'{state,state}' <> 'proofs_reserved')
           OR (NOT initial AND p_saga#>>'{state,state}' <> 'melt_requested')
           OR p_saga#>>'{data,kind}' <> 'melt' OR (initial AND p_counter <> 'null'::JSONB) THEN
            RAISE EXCEPTION 'Invalid melt preparation';
        END IF;
    END IF;
    IF expected_quotes <> COALESCE((SELECT pg_catalog.jsonb_agg(value->'quote_id') FROM pg_catalog.jsonb_array_elements(p_quotes)), '[]'::JSONB)
       OR (NOT is_swap AND pg_catalog.jsonb_array_length(expected_quotes) = 0) THEN
        RAISE EXCEPTION 'Invalid preparation quotes';
    END IF;
    IF NOT initial THEN
        UPDATE public.saga SET data = p_saga::TEXT, version = (p_saga->>'version')::INTEGER,
            updated_at = (p_saga->>'updated_at')::BIGINT
        WHERE wallet_id = wallet AND id = operation AND version = (p_saga->>'version')::INTEGER - 1
            AND data::JSONB#>>'{state,state}' = 'proofs_reserved'
            AND data::JSONB->>'kind' = 'melt'
            AND data::JSONB->>'mint_url' = mint AND data::JSONB->>'unit' = operation_unit
            AND data::JSONB->>'quote_id' = p_saga->>'quote_id';
        GET DIAGNOSTICS affected = ROW_COUNT;
        IF affected <> 1 THEN RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'Preparation saga conflict'; END IF;
    END IF;
    IF is_swap AND parent IS NOT NULL AND parent <> 'null'::JSONB THEN
        UPDATE public.saga SET version = version + 1,
            data = pg_catalog.jsonb_set(data::JSONB, '{version}', pg_catalog.to_jsonb(version + 1))::TEXT
        WHERE wallet_id = wallet AND id = parent->>'id' AND version = (parent->>'version')::INTEGER
            AND data::JSONB#>>'{state,state}' = 'proofs_reserved' AND data::JSONB->>'kind' = 'melt'
            AND data::JSONB->>'mint_url' = mint AND data::JSONB->>'unit' = operation_unit;
        GET DIAGNOSTICS affected = ROW_COUNT;
        IF affected <> 1 THEN RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'Preparation parent conflict'; END IF;
    END IF;
    FOR q IN SELECT value FROM pg_catalog.jsonb_array_elements(p_quotes) LOOP
        IF is_mint THEN
            UPDATE public.mint_quote SET used_by_operation = operation, version = version + 1
            WHERE wallet_id = wallet AND id = q->>'quote_id' AND version = (q->>'version')::INTEGER
              AND used_by_operation IS NULL AND mint_url = mint AND mint_quote.unit = operation_unit;
        ELSE
            UPDATE public.melt_quote SET used_by_operation = operation, version = version + CASE WHEN initial THEN 1 ELSE 0 END
            WHERE wallet_id = wallet AND id = q->>'quote_id' AND version = (q->>'version')::INTEGER
              AND ((initial AND used_by_operation IS NULL) OR (NOT initial AND used_by_operation = operation)) AND (mint_url = mint OR mint_url IS NULL)
              AND melt_quote.unit = operation_unit AND state = 'UNPAID';
        END IF;
        GET DIAGNOSTICS affected = ROW_COUNT;
        IF affected <> 1 THEN RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'Preparation quote conflict'; END IF;
    END LOOP;
    -- Existing inputs and imports use the same locked ownership check.
    FOR p IN
        SELECT pg_catalog.jsonb_build_object('y', value) FROM pg_catalog.jsonb_array_elements(p_proof_ys)
        UNION ALL SELECT value FROM pg_catalog.jsonb_array_elements(p_proofs)
    LOOP
        input_y := p->>'y';
        SELECT * INTO existing FROM public.proof WHERE wallet_id = wallet AND y = input_y FOR UPDATE;
        IF FOUND THEN
            IF (is_swap AND parent IS NOT NULL AND parent <> 'null'::JSONB AND (existing.state <> 'RESERVED' OR existing.used_by_operation IS DISTINCT FROM parent->>'id'))
               OR (initial AND (NOT is_swap OR parent IS NULL OR parent = 'null'::JSONB) AND (existing.state <> 'UNSPENT' OR existing.used_by_operation IS NOT NULL))
               OR (NOT initial AND (existing.state <> 'RESERVED' OR existing.used_by_operation IS DISTINCT FROM operation))
               OR existing.mint_url <> mint OR existing.unit <> operation_unit
               OR (p ? 'amount' AND (existing.amount <> (p->>'amount')::BIGINT OR existing.keyset_id <> p->>'keyset_id')) THEN
                RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'Preparation input conflict';
            END IF;
            UPDATE public.proof SET state = CASE WHEN initial THEN 'RESERVED' ELSE 'PENDING' END, used_by_operation = operation,
                witness = CASE WHEN p ? 'amount' THEN p->>'witness' ELSE witness END
            WHERE wallet_id = wallet AND y = input_y;
        ELSE
            IF NOT initial OR NOT (p ? 'amount') OR p->>'mint_url' <> mint OR p->>'unit' <> operation_unit
               OR p->>'state' <> 'UNSPENT' OR p->>'used_by_operation' IS NOT NULL THEN
                RAISE EXCEPTION 'Invalid preparation input';
            END IF;
            INSERT INTO public.proof (y, wallet_id, mint_url, state, spending_condition, unit, amount,
                keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, used_by_operation,
                created_by_operation, derivation_index, p2pk_e)
            VALUES (input_y, wallet, mint, 'RESERVED', p->>'spending_condition', operation_unit, (p->>'amount')::BIGINT,
                p->>'keyset_id', p->>'secret', p->>'c', p->>'witness', p->>'dleq_e', p->>'dleq_s', p->>'dleq_r',
                operation, p->>'created_by_operation', (p->>'derivation_index')::INTEGER, p->>'p2pk_e');
        END IF;
    END LOOP;
    IF p_counter IS NOT NULL AND p_counter <> 'null'::JSONB THEN
        IF (p_counter->>'end')::BIGINT < (p_counter->>'start')::BIGINT
           OR p_counter->'start' <> p_saga#>'{data,data,counter_start}'
           OR p_counter->'end' <> p_saga#>'{data,data,counter_end}' THEN
            RAISE EXCEPTION 'Invalid preparation counter';
        END IF;
        INSERT INTO public.keyset_counter (keyset_id, wallet_id, counter)
        VALUES (p_counter->>'keyset_id', wallet, 0) ON CONFLICT DO NOTHING;
        UPDATE public.keyset_counter SET counter = (p_counter->>'end')::INTEGER
        WHERE wallet_id = wallet AND keyset_id = p_counter->>'keyset_id' AND counter = (p_counter->>'start')::INTEGER;
        GET DIAGNOSTICS affected = ROW_COUNT;
        IF affected <> 1 THEN RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'Preparation counter conflict'; END IF;
    END IF;
    IF initial THEN
    INSERT INTO public.saga (id, wallet_id, data, version, completed, created_at, updated_at)
    VALUES (operation, wallet, p_saga::TEXT, 0, FALSE, (p_saga->>'created_at')::BIGINT, (p_saga->>'updated_at')::BIGINT);
    END IF;
END;
$body$;

INSERT INTO public.schema_info (key, value) VALUES ('schema_version', '11')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
