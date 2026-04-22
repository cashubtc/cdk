-- Add unique index to request column in mint_quote table
CREATE UNIQUE INDEX IF NOT EXISTS idx_mint_quote_request_unique ON mint_quote(request);
