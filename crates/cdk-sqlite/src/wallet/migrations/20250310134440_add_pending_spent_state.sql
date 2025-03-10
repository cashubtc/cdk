
-- Add PENDING_SPENT state to proof table
ALTER TABLE proof
    ADD CONSTRAINT proof_state_check CHECK (state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 'PENDING_SPENT'));
