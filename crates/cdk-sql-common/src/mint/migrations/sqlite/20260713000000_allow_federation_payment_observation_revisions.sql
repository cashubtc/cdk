DROP INDEX IF EXISTS idx_federation_payment_observations_member_state;

CREATE INDEX idx_federation_payment_observations_member_state
ON federation_payment_observations (context, quote, observer_member_id, state);
