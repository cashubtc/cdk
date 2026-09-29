-- NUT-XX transaction records, keyed by transaction digest
CREATE TABLE IF NOT EXISTS transactions (
    digest TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    unit TEXT NOT NULL,
    melt_quote_id TEXT,
    quote_inputs TEXT NOT NULL,
    change_pubkey TEXT,
    excess BIGINT NOT NULL,
    change_quote_id TEXT,
    operation_id TEXT NOT NULL,
    created_time BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_transactions_melt_quote_id ON transactions(melt_quote_id);
