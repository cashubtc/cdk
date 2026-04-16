-- Add unique index to request column in mint_quote table
CREATE UNIQUE INDEX IF NOT EXISTS unique_mint_quote_request ON mint_quote(request);
