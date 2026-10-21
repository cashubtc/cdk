-- Fixed txout quotes a NUT-XX transaction creates on settlement
ALTER TABLE melt_transaction ADD COLUMN mint_quote_outputs TEXT NOT NULL DEFAULT '[]';

-- A payment request names one quote; txout quotes share the empty request.
DROP INDEX IF EXISTS idx_mint_quote_request_unique;
CREATE UNIQUE INDEX IF NOT EXISTS idx_mint_quote_request_unique_nonempty ON mint_quote(request) WHERE request <> '';
CREATE INDEX IF NOT EXISTS idx_mint_quote_request ON mint_quote(request);

-- Fixed txout quote ids claimed by accepted NUT-XX transactions
CREATE TABLE IF NOT EXISTS mint_quote_id_claim (
    quote_id TEXT PRIMARY KEY,
    melt_quote_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_mint_quote_id_claim_melt_quote_id ON mint_quote_id_claim(melt_quote_id);
