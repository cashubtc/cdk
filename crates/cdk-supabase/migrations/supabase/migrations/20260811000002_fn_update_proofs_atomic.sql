-- Recreate the atomic proof update RPC with all current proof metadata.
CREATE OR REPLACE FUNCTION update_proofs_atomic(
    p_proofs_to_add JSONB DEFAULT '[]'::JSONB,
    p_ys_to_remove JSONB DEFAULT '[]'::JSONB
)
RETURNS JSONB
LANGUAGE sql
SECURITY DEFINER
SET search_path = ''
AS $body$
    WITH
    removed AS (
        DELETE FROM public.proof
        WHERE wallet_id = public.get_current_wallet_id()
          AND y = ANY(SELECT pg_catalog.jsonb_array_elements_text(p_ys_to_remove))
        RETURNING y
    ),
    inserted AS (
        INSERT INTO public.proof AS existing (
            y, wallet_id, mint_url, state, spending_condition, unit, amount,
            keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r,
            used_by_operation, created_by_operation, p2pk_e, derivation_index
        )
        SELECT
            p->>'y',
            public.get_current_wallet_id(),
            p->>'mint_url',
            p->>'state',
            p->>'spending_condition',
            p->>'unit',
            (p->>'amount')::BIGINT,
            p->>'keyset_id',
            p->>'secret',
            p->>'c',
            p->>'witness',
            p->>'dleq_e',
            p->>'dleq_s',
            p->>'dleq_r',
            p->>'used_by_operation',
            p->>'created_by_operation',
            p->>'p2pk_e',
            (p->>'derivation_index')::INTEGER
        FROM pg_catalog.jsonb_array_elements(p_proofs_to_add) AS p
        ON CONFLICT (y, wallet_id) DO UPDATE SET
            mint_url = EXCLUDED.mint_url,
            state = EXCLUDED.state,
            spending_condition = EXCLUDED.spending_condition,
            unit = EXCLUDED.unit,
            amount = EXCLUDED.amount,
            keyset_id = EXCLUDED.keyset_id,
            secret = EXCLUDED.secret,
            c = EXCLUDED.c,
            witness = EXCLUDED.witness,
            dleq_e = EXCLUDED.dleq_e,
            dleq_s = EXCLUDED.dleq_s,
            dleq_r = EXCLUDED.dleq_r,
            used_by_operation = EXCLUDED.used_by_operation,
            created_by_operation = EXCLUDED.created_by_operation,
            p2pk_e = EXCLUDED.p2pk_e,
            derivation_index = COALESCE(
                EXCLUDED.derivation_index,
                existing.derivation_index
            )
        RETURNING y
    )
    SELECT pg_catalog.jsonb_build_object(
        'added',   (SELECT count(*) FROM inserted),
        'removed', (SELECT count(*) FROM removed)
    )
$body$;
