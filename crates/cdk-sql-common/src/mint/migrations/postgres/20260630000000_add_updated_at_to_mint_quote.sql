ALTER TABLE mint_quote ADD COLUMN IF NOT EXISTS updated_at BIGINT NOT NULL DEFAULT 0;

UPDATE mint_quote
SET updated_at = GREATEST(
    created_time,
    COALESCE((SELECT MAX(timestamp) FROM mint_quote_payments WHERE quote_id = mint_quote.id), created_time),
    COALESCE((SELECT MAX(timestamp) FROM mint_quote_issued WHERE quote_id = mint_quote.id), created_time)
);
