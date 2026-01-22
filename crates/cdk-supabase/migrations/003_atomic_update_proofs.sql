-- CDK Wallet Database Schema Migration 003: Atomic Update Proofs
-- This migration adds an RPC function for atomic proof updates (add + remove in single transaction)

-- Function: update_proofs_atomic
-- Atomically adds new proofs and removes proofs by Y values in a single transaction
-- Parameters:
--   p_proofs_to_add: JSON array of proof objects to insert/upsert
--   p_ys_to_remove: JSON array of Y values (hex strings) to delete
-- Returns: JSON object with counts of added and removed proofs
CREATE OR REPLACE FUNCTION update_proofs_atomic(
    p_proofs_to_add JSONB DEFAULT '[]'::JSONB,
    p_ys_to_remove JSONB DEFAULT '[]'::JSONB
)
RETURNS JSONB
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
    added_count INTEGER := 0;
    removed_count INTEGER := 0;
    proof_record JSONB;
BEGIN
    -- Remove proofs by Y values first (to handle any conflicts)
    IF jsonb_array_length(p_ys_to_remove) > 0 THEN
        DELETE FROM proof
        WHERE y = ANY(SELECT jsonb_array_elements_text(p_ys_to_remove))
        RETURNING 1 INTO removed_count;
        
        GET DIAGNOSTICS removed_count = ROW_COUNT;
    END IF;
    
    -- Add/upsert new proofs
    IF jsonb_array_length(p_proofs_to_add) > 0 THEN
        FOR proof_record IN SELECT * FROM jsonb_array_elements(p_proofs_to_add)
        LOOP
            INSERT INTO proof (
                y, mint_url, state, spending_condition, unit, amount,
                keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r
            )
            VALUES (
                proof_record->>'y',
                proof_record->>'mint_url',
                proof_record->>'state',
                proof_record->>'spending_condition',
                proof_record->>'unit',
                (proof_record->>'amount')::BIGINT,
                proof_record->>'keyset_id',
                proof_record->>'secret',
                proof_record->>'c',
                proof_record->>'witness',
                proof_record->>'dleq_e',
                proof_record->>'dleq_s',
                proof_record->>'dleq_r'
            )
            ON CONFLICT (y) DO UPDATE SET
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
                dleq_r = EXCLUDED.dleq_r;
            
            added_count := added_count + 1;
        END LOOP;
    END IF;
    
    RETURN jsonb_build_object(
        'added', added_count,
        'removed', removed_count
    );
END;
$$;

-- Grant execute permission
GRANT EXECUTE ON FUNCTION update_proofs_atomic(JSONB, JSONB) TO authenticated;
GRANT EXECUTE ON FUNCTION update_proofs_atomic(JSONB, JSONB) TO service_role;

-- Add a comment describing the function
COMMENT ON FUNCTION update_proofs_atomic(JSONB, JSONB) IS 
    'Atomically adds and removes proofs in a single transaction. '
    'p_proofs_to_add is a JSON array of proof objects to upsert. '
    'p_ys_to_remove is a JSON array of Y value strings (hex) to delete. '
    'Returns JSON with added and removed counts.';
