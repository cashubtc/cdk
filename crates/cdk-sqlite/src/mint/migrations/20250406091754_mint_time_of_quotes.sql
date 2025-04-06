-- Add timestamp columns to mint_quote table
ALTER TABLE mint_quote ADD COLUMN created_time INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN paid_time INTEGER;
ALTER TABLE mint_quote ADD COLUMN issued_time INTEGER;

-- Add timestamp columns to melt_quote table
ALTER TABLE melt_quote ADD COLUMN created_time INTEGER NOT NULL DEFAULT 0;
ALTER TABLE melt_quote ADD COLUMN paid_time INTEGER;
