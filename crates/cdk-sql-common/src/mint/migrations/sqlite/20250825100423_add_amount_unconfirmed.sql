-- Add amount_unconfirmed column to mint_quote table
ALTER TABLE mint_quote ADD COLUMN amount_unconfirmed BIGINT NOT NULL DEFAULT 0;
