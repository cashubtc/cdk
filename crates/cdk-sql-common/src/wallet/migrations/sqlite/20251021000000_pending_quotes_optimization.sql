-- Add created_time column to mint_quote table for ordering queries
ALTER TABLE mint_quote ADD COLUMN created_time INTEGER NOT NULL DEFAULT (strftime('%s', 'now'));

-- Composite index for optimized pending quotes query
-- Supports WHERE (amount_paid > amount_issued) OR payment_method = 'bolt12'
CREATE INDEX IF NOT EXISTS idx_mint_quote_pending
ON mint_quote(payment_method, amount_paid, amount_issued);
