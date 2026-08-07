ALTER TABLE proof ADD COLUMN IF NOT EXISTS derivation_index INTEGER;

CREATE INDEX IF NOT EXISTS proof_keyset_state_derivation_index
ON proof(keyset_id, state, derivation_index);
