-- Fixed txout quotes a NUT-XX transaction creates on settlement
ALTER TABLE melt_transaction ADD COLUMN mint_quote_outputs TEXT NOT NULL DEFAULT '[]';

-- A payment request names one quote; txout quotes share the empty request.
DROP INDEX IF EXISTS unique_mint_quote_request;
CREATE UNIQUE INDEX IF NOT EXISTS unique_mint_quote_request_nonempty ON mint_quote(request) WHERE request <> '';
CREATE INDEX IF NOT EXISTS idx_mint_quote_request ON mint_quote(request);
