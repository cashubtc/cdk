CREATE TABLE IF NOT EXISTS federation_payment_observations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_index BIGINT NOT NULL,
    context TEXT NOT NULL,
    quote TEXT NOT NULL,
    observer_member_id INTEGER NOT NULL,
    state TEXT NOT NULL,
    observation_json TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_federation_payment_observations_member_state
ON federation_payment_observations (context, quote, observer_member_id, state);

CREATE INDEX IF NOT EXISTS idx_federation_payment_observations_quote
ON federation_payment_observations (context, quote, operation_index);
