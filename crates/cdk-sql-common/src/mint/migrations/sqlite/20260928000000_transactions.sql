-- NUT-XX transaction records, keyed by transaction digest
CREATE TABLE IF NOT EXISTS transactions (
    digest TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    unit TEXT NOT NULL,
    melt_quote_id TEXT,
    quote_inputs TEXT NOT NULL,
    change_outputs TEXT NOT NULL,
    excess INTEGER NOT NULL,
    change_quote_ids TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    created_time INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_transactions_melt_quote_id ON transactions(melt_quote_id);

-- Every change quote of a transaction carries the digest as its request, so
-- request is no longer unique; the lookup by request only ever sees invoices.
DROP INDEX IF EXISTS idx_mint_quote_request_unique;
CREATE INDEX IF NOT EXISTS idx_mint_quote_request ON mint_quote(request);
