-- Settlement state of NUT-XX transactions with a melt, keyed by melt quote
CREATE TABLE IF NOT EXISTS melt_transaction (
    melt_quote_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    unit TEXT NOT NULL,
    quote_inputs TEXT NOT NULL,
    mint_quote_outputs TEXT NOT NULL,
    excess INTEGER NOT NULL,
    created_time INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_melt_transaction_state ON melt_transaction(state);

-- A payment request names one quote; txout quotes share the empty request.
DROP INDEX IF EXISTS idx_mint_quote_request_unique;
CREATE UNIQUE INDEX IF NOT EXISTS idx_mint_quote_request_unique_nonempty ON mint_quote(request) WHERE request <> '';
CREATE INDEX IF NOT EXISTS idx_mint_quote_request ON mint_quote(request);
