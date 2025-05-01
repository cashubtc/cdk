-- Add new columns to mint_quote table
ALTER TABLE mint_quote ADD COLUMN amount_paid INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN amount_minted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN payment_method TEXT NOT NULL DEFAULT 'BOLT11';

-- Remove NOT NULL constraint from amount column
PRAGMA foreign_keys=off;
CREATE TABLE mint_quote_new (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    payment_method TEXT NOT NULL DEFAULT 'BOLT11',
    amount INTEGER,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    state TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    amount_paid INTEGER NOT NULL DEFAULT 0,
    amount_minted INTEGER NOT NULL DEFAULT 0,
    secret_key TEXT
);

-- Explicitly specify columns for proper mapping
INSERT INTO mint_quote_new (
    id, 
    mint_url, 
    payment_method,
    amount, 
    unit, 
    request, 
    state, 
    expiry, 
    amount_paid,
    amount_minted,
    secret_key
) 
SELECT 
    id, 
    mint_url, 
    'bolt11', -- Default value for the new payment_method column
    amount, 
    unit, 
    request, 
    state, 
    expiry, 
    0, -- Default value for amount_paid
    0, -- Default value for amount_minted
    secret_key
FROM mint_quote;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;
PRAGMA foreign_keys=on;

-- Set amount_paid equal to amount for quotes with PAID or ISSUED state
UPDATE mint_quote SET amount_paid = amount WHERE state = 'PAID' OR state = 'ISSUED';
-- Set amount_minted equal to amount for quotes with ISSUED state
UPDATE mint_quote SET amount_minted = amount WHERE state = 'ISSUED';
