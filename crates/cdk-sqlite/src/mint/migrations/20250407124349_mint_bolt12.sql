

-- Add new columns to mint_quote table
ALTER TABLE mint_quote ADD COLUMN amount_paid INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN amount_minted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mint_quote ADD COLUMN payment_method TEXT NOT NULL DEFAULT 'bolt11';
ALTER TABLE mint_quote ADD COLUMN pending BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE mint_quote DROP COLUMN issued_time;
ALTER TABLE mint_quote DROP COLUMN paid_time;

-- Update pending column based on state
UPDATE mint_quote SET pending = TRUE WHERE state = 'PENDING';

-- Set amount_paid equal to amount for quotes with PAID or ISSUED state
UPDATE mint_quote SET amount_paid = amount WHERE state = 'PAID' OR state = 'ISSUED';

-- Set amount_minted equal to amount for quotes with ISSUED state
UPDATE mint_quote SET amount_minted = amount WHERE state = 'ISSUED';

DROP INDEX IF EXISTS mint_quote_state_index;

-- Remove the state column from mint_quote table
ALTER TABLE mint_quote DROP COLUMN state;

-- Remove NOT NULL constraint from amount column
CREATE TABLE mint_quote_temp (
    id TEXT PRIMARY KEY,
    amount INTEGER,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    request_lookup_id TEXT UNIQUE,
    pubkey TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    amount_paid INTEGER NOT NULL DEFAULT 0,
    amount_minted INTEGER NOT NULL DEFAULT 0,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    pending BOOLEAN NOT NULL DEFAULT FALSE
);

INSERT INTO mint_quote_temp (id, amount, unit, request, expiry, request_lookup_id, pubkey, created_time, amount_paid, amount_minted, payment_method, pending) SELECT id, amount, unit, request, expiry, request_lookup_id, pubkey, created_time, amount_paid, amount_minted, payment_method, pending FROM mint_quote;
DROP TABLE mint_quote;
ALTER TABLE mint_quote_temp RENAME TO mint_quote;

-- Create mint_quote_payments table
CREATE TABLE mint_quote_payments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    quote_id TEXT NOT NULL,
    payment_id TEXT NOT NULL UNIQUE,
    timestamp INTEGER NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);

-- Create index on payment_id for faster lookups
CREATE INDEX idx_mint_quote_payments_payment_id ON mint_quote_payments(payment_id);

-- Create mint_quote_issued table
CREATE TABLE mint_quote_issued (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    quote_id TEXT NOT NULL,
    amount INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);

-- Create index on quote_id for faster lookups
CREATE INDEX idx_mint_quote_issued_quote_id ON mint_quote_issued(quote_id);


-- Add new columns to melt_quote table
ALTER TABLE melt_quote ADD COLUMN payment_method TEXT NOT NULL DEFAULT 'bolt11';
ALTER TABLE melt_quote ADD COLUMN options TEXT;
