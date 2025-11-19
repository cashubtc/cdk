-- Add fee_collected column to keyset_amounts table
ALTER TABLE keyset_amounts ADD COLUMN fee_collected BIGINT NOT NULL DEFAULT 0;
