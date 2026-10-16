-- Fixed txout quote ids claimed by accepted NUT-XX transactions
CREATE TABLE IF NOT EXISTS mint_quote_id_claim (
    quote_id TEXT PRIMARY KEY,
    melt_quote_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_mint_quote_id_claim_melt_quote_id ON mint_quote_id_claim(melt_quote_id);
