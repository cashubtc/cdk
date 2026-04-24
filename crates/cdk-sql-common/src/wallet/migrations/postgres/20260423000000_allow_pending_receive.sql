-- Migration to add PENDING_RECEIVE to the proof state check constraint
-- Since the constraint is anonymous in initial migration, we drop and recreate it with a name

-- Drop potential anonymous constraints (Postgres generates names like proof_state_check)
DO $$ 
BEGIN 
    ALTER TABLE proof DROP CONSTRAINT IF EXISTS proof_state_check;
EXCEPTION 
    WHEN undefined_object THEN null; 
END $$;

ALTER TABLE proof ADD CONSTRAINT proof_state_check CHECK (state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 'PENDING_SPENT', 'PENDING_RECEIVE'));
