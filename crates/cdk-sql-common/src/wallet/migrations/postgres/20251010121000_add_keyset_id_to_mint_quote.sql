-- Add keyset_id column to mint_quote table for mining share quotes
ALTER TABLE mint_quote ADD COLUMN keyset_id TEXT;
