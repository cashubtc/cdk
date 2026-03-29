-- RPC: atomically delete + upsert proofs in one CTE, return counts
CREATE OR REPLACE FUNCTION update_proofs_atomic(
    p_proofs_to_add JSONB DEFAULT '[]'::JSONB,
    p_ys_to_remove JSONB DEFAULT '[]'::JSONB
)
RETURNS JSONB
LANGUAGE sql
SECURITY DEFINER
AS $body$
    WITH
    removed AS (
        DELETE FROM proof
        WHERE wallet_id = public.get_current_wallet_id()
          AND y = ANY(SELECT jsonb_array_elements_text(p_ys_to_remove))
        RETURNING y
    ),
    inserted AS (
        INSERT INTO proof (
            y, wallet_id, mint_url, state, spending_condition, unit, amount,
            keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r
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
            p->>'dleq_r'
        FROM jsonb_array_elements(p_proofs_to_add) AS p
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
            dleq_r = EXCLUDED.dleq_r
        RETURNING y
    )
    SELECT jsonb_build_object(
        'added',   (SELECT count(*) FROM inserted),
        'removed', (SELECT count(*) FROM removed)
    )
$body$;
