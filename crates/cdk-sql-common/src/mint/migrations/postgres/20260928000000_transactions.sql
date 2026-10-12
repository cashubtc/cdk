-- Settlement state of NUT-XX transactions with a melt, keyed by melt quote
CREATE TABLE IF NOT EXISTS melt_transaction (
    melt_quote_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    unit TEXT NOT NULL,
    quote_inputs TEXT NOT NULL,
    change_outputs TEXT NOT NULL,
    excess BIGINT NOT NULL,
    created_time BIGINT NOT NULL
);

-- Mint quote ids claimed by accepted NUT-XX transactions
CREATE TABLE IF NOT EXISTS mint_quote_id_claim (
    quote_id TEXT PRIMARY KEY,
    melt_quote_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_mint_quote_id_claim_melt_quote_id ON mint_quote_id_claim(melt_quote_id);

-- A payment request names one quote; change quotes share the empty request.
DROP INDEX IF EXISTS unique_mint_quote_request;
CREATE UNIQUE INDEX IF NOT EXISTS unique_mint_quote_request_nonempty ON mint_quote(request) WHERE request <> '';
CREATE INDEX IF NOT EXISTS idx_mint_quote_request ON mint_quote(request);
