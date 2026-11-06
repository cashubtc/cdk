-- Settlement state of NUT-XX transactions with a melt, keyed by melt quote
CREATE TABLE IF NOT EXISTS melt_transaction (
    melt_quote_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    unit TEXT NOT NULL,
    quote_inputs TEXT NOT NULL,
    excess INTEGER NOT NULL,
    created_time INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_melt_transaction_state ON melt_transaction(state);

