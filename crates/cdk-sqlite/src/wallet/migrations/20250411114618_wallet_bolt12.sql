-- Add new columns to mint_quote table
ALTER TABLE mint_quote ADD COLUMN amount_paid INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN amount_minted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN payment_method TEXT NOT NULL DEFAULT 'BOLT11';
ALTER TABLE mint_quote ADD COLUMN single_use BOOLEAN NOT NULL DEFAULT TRUE;

-- Set amount_paid equal to amount for quotes with PAID or ISSUED state
UPDATE mint_quote SET amount_paid = amount WHERE state = 'PAID' OR state = 'ISSUED';

-- Set amount_minted equal to amount for quotes with ISSUED state
UPDATE mint_quote SET amount_minted = amount WHERE state = 'ISSUED';
