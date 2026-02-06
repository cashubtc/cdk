-- Remove blinded_secrets and input_ys columns from saga_state table
-- These values can be looked up from proof and blind_signature tables using operation_id

ALTER TABLE saga_state DROP COLUMN IF EXISTS blinded_secrets;
ALTER TABLE saga_state DROP COLUMN IF EXISTS input_ys;
