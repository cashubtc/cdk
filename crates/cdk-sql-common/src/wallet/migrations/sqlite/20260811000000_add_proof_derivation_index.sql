ALTER TABLE proof ADD COLUMN derivation_index INTEGER;

CREATE INDEX proof_keyset_state_derivation_index
ON proof(keyset_id, state, derivation_index);
