-- Run against an isolated database with the wallet migrations applied.
-- The caller must have permission to SET ROLE authenticated.
BEGIN;
SET LOCAL ROLE authenticated;
SELECT set_config('request.jwt.claims', '{"sub":"preparation-contract"}', true);
DO $test$
DECLARE
    mint TEXT := 'https://preparation.example.com';
    initial JSONB := '{"id":"melt-operation","kind":"melt","state":{"type":"melt","state":"proofs_reserved"},"amount":1000,"mint_url":"https://preparation.example.com","unit":"sat","quote_id":"melt-quote","created_at":0,"updated_at":0,"version":0,"data":{"kind":"melt","data":{"quote_id":"melt-quote","amount":1000,"fee_reserve":10,"counter_start":null,"counter_end":null,"change_amount":null}}}';
    child JSONB := '{"id":"swap-operation","kind":"swap","state":{"type":"swap","state":"proofs_reserved"},"amount":2048,"mint_url":"https://preparation.example.com","unit":"sat","quote_id":null,"created_at":0,"updated_at":0,"version":0,"data":{"kind":"swap","data":{"parent":{"id":"melt-operation","version":0},"input_amount":2048,"output_amount":2048,"counter_start":0,"counter_end":1,"blinded_messages":null}}}';
    requested JSONB;
    issue JSONB := '{"id":"mint-operation","kind":"mint","state":{"type":"issue","state":"secrets_prepared"},"amount":2,"mint_url":"https://preparation.example.com","unit":"sat","quote_id":"mint-a","created_at":0,"updated_at":0,"version":0,"data":{"kind":"mint","data":{"quote_id":"mint-a","quote_ids":["mint-a","mint-b"],"is_batch":true,"amount":2,"counter_start":0,"counter_end":1,"blinded_messages":[]}}}';
    rejected BOOLEAN;
BEGIN
    INSERT INTO public.melt_quote (id, unit, amount, request, fee_reserve, state, expiry, payment_method, mint_url)
    VALUES ('melt-quote', 'sat', 1000, 'test request', 10, 'UNPAID', 9999999999, 'bolt11', mint);
    INSERT INTO public.proof (y, mint_url, state, unit, amount, keyset_id, secret, c)
    VALUES ('input', mint, 'UNSPENT', 'sat', 2048, 'test-keyset', 'encrypted test secret', 'encrypted test signature');
    INSERT INTO public.saga (id, data) VALUES ('melt-operation', '{}');
    rejected := FALSE;
    BEGIN
        PERFORM public.prepare_wallet_operation(initial, '["input"]', '[]', '[{"quote_id":"melt-quote","version":0}]', 'null');
    EXCEPTION WHEN unique_violation THEN rejected := TRUE;
    END;
    ASSERT rejected;
    ASSERT (SELECT state = 'UNSPENT' AND used_by_operation IS NULL FROM public.proof WHERE y = 'input');
    ASSERT (SELECT used_by_operation IS NULL AND version = 0 FROM public.melt_quote WHERE id = 'melt-quote');
    DELETE FROM public.saga WHERE id = 'melt-operation';
    rejected := FALSE;
    BEGIN
        PERFORM public.prepare_wallet_operation(initial, '["input","missing"]', '[]', '[{"quote_id":"melt-quote","version":0}]', 'null');
    EXCEPTION WHEN raise_exception THEN rejected := TRUE;
    END;
    ASSERT rejected;
    ASSERT (SELECT state = 'UNSPENT' AND used_by_operation IS NULL FROM public.proof WHERE y = 'input');
    ASSERT (SELECT used_by_operation IS NULL AND version = 0 FROM public.melt_quote WHERE id = 'melt-quote');
    PERFORM public.prepare_wallet_operation(initial, '["input"]', '[]', '[{"quote_id":"melt-quote","version":0}]', 'null');
    ASSERT (SELECT state = 'RESERVED' AND used_by_operation = 'melt-operation' FROM public.proof WHERE y = 'input');
    ASSERT (SELECT used_by_operation = 'melt-operation' AND version = 1 FROM public.melt_quote WHERE id = 'melt-quote');
    ASSERT (SELECT data::JSONB = initial FROM public.saga WHERE id = 'melt-operation');

    INSERT INTO public.saga (id, data) VALUES ('swap-operation', '{}');
    rejected := FALSE;
    BEGIN
        PERFORM public.prepare_wallet_operation(child, '["input"]', '[]', '[]', '{"keyset_id":"test-keyset","start":0,"end":1}');
    EXCEPTION WHEN unique_violation THEN rejected := TRUE;
    END;
    ASSERT rejected;
    ASSERT (SELECT version = 0 FROM public.saga WHERE id = 'melt-operation');
    ASSERT (SELECT used_by_operation = 'melt-operation' FROM public.proof WHERE y = 'input');
    ASSERT NOT EXISTS (SELECT 1 FROM public.keyset_counter);
    DELETE FROM public.saga WHERE id = 'swap-operation';
    PERFORM public.prepare_wallet_operation(child, '["input"]', '[]', '[]', '{"keyset_id":"test-keyset","start":0,"end":1}');
    ASSERT (SELECT version = 1 AND data::JSONB->>'version' = '1' FROM public.saga WHERE id = 'melt-operation');
    ASSERT (SELECT used_by_operation = 'swap-operation' AND state = 'RESERVED' FROM public.proof WHERE y = 'input');
    ASSERT (SELECT counter = 1 FROM public.keyset_counter WHERE keyset_id = 'test-keyset');

    -- Exercise the later melt preparation and its counter conflict rollback.
    UPDATE public.proof SET used_by_operation = 'melt-operation' WHERE y = 'input';
    DELETE FROM public.saga WHERE id = 'swap-operation';
    UPDATE public.keyset_counter SET counter = 2 WHERE keyset_id = 'test-keyset';
    requested := initial || '{"version":2,"state":{"type":"melt","state":"melt_requested"}}'::JSONB;
    requested := pg_catalog.jsonb_set(requested, '{data,data}', requested#>'{data,data}' || '{"counter_start":1,"counter_end":2,"final_proof_ys":["input"]}'::JSONB);
    rejected := FALSE;
    BEGIN
        PERFORM public.prepare_wallet_operation(requested, '["input"]', '[]', '[{"quote_id":"melt-quote","version":1}]', '{"keyset_id":"test-keyset","start":1,"end":2}');
    EXCEPTION WHEN unique_violation THEN rejected := TRUE;
    END;
    ASSERT rejected;
    ASSERT (SELECT state = 'RESERVED' FROM public.proof WHERE y = 'input');
    ASSERT (SELECT version = 1 AND data::JSONB#>>'{state,state}' = 'proofs_reserved' FROM public.saga WHERE id = 'melt-operation');
    requested := pg_catalog.jsonb_set(requested, '{data,data}', requested#>'{data,data}' || '{"counter_start":2,"counter_end":3}'::JSONB);
    PERFORM public.prepare_wallet_operation(requested, '["input"]', '[]', '[{"quote_id":"melt-quote","version":1}]', '{"keyset_id":"test-keyset","start":2,"end":3}');
    ASSERT (SELECT state = 'PENDING' AND used_by_operation = 'melt-operation' FROM public.proof WHERE y = 'input');
    ASSERT (SELECT version = 2 FROM public.saga WHERE id = 'melt-operation');
    ASSERT (SELECT counter = 3 FROM public.keyset_counter WHERE keyset_id = 'test-keyset');
    rejected := FALSE;
    BEGIN
        PERFORM public.prepare_wallet_operation(requested, '["input"]', '[]', '[{"quote_id":"melt-quote","version":1}]', '{"keyset_id":"test-keyset","start":2,"end":3}');
    EXCEPTION WHEN unique_violation THEN rejected := TRUE;
    END;
    ASSERT rejected;

    INSERT INTO public.mint_quote (id, mint_url, amount, unit, state, expiry, payment_method, amount_paid)
    VALUES ('mint-a', mint, 1, 'sat', 'PAID', 9999999999, 'bolt12', 1), ('mint-b', mint, 1, 'sat', 'PAID', 9999999999, 'bolt12', 1);
    UPDATE public.mint_quote SET used_by_operation = 'other' WHERE id = 'mint-b';
    rejected := FALSE;
    BEGIN
        PERFORM public.prepare_wallet_operation(issue, '[]', '[]', '[{"quote_id":"mint-a","version":0},{"quote_id":"mint-b","version":0}]', '{"keyset_id":"mint-keyset","start":0,"end":1}');
    EXCEPTION WHEN unique_violation THEN rejected := TRUE;
    END;
    ASSERT rejected;
    ASSERT (SELECT used_by_operation IS NULL AND version = 0 FROM public.mint_quote WHERE id = 'mint-a');
    ASSERT NOT EXISTS (SELECT 1 FROM public.keyset_counter WHERE keyset_id = 'mint-keyset');
    UPDATE public.mint_quote SET used_by_operation = NULL WHERE id = 'mint-b';
    PERFORM public.prepare_wallet_operation(issue, '[]', '[]', '[{"quote_id":"mint-a","version":0},{"quote_id":"mint-b","version":0}]', '{"keyset_id":"mint-keyset","start":0,"end":1}');
    ASSERT (SELECT count(*) = 2 FROM public.mint_quote WHERE used_by_operation = 'mint-operation' AND version = 1);
    ASSERT (SELECT counter = 1 FROM public.keyset_counter WHERE keyset_id = 'mint-keyset');

    PERFORM set_config('request.jwt.claims', '{"sub":"different-wallet"}', true);
    ASSERT NOT EXISTS (SELECT 1 FROM public.proof);
    ASSERT NOT EXISTS (SELECT 1 FROM public.saga);
    ASSERT NOT EXISTS (SELECT 1 FROM public.mint_quote);
    ASSERT NOT EXISTS (SELECT 1 FROM public.melt_quote);
END;
$test$;
ROLLBACK;
