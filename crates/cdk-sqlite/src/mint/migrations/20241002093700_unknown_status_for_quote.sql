-- Create a new table with the updated CHECK constraint
CREATE TABLE melt_quote_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    request TEXT NOT NULL,
    fee_reserve INTEGER NOT NULL,
    expiry INTEGER NOT NULL,
    state TEXT CHECK ( state IN ('UNPAID', 'PENDING', 'PAID', 'UNKNOWN') ) NOT NULL DEFAULT 'UNPAID',
    payment_preimage TEXT,
    request_lookup_id TEXT
);

-- Copy the data from the old table to the new table
INSERT INTO melt_quote_new (id, unit, amount, request, fee_reserve, expiry, state, payment_preimage, request_lookup_id)
SELECT id, unit, amount, request, fee_reserve, expiry, state, payment_preimage, request_lookup_id
FROM melt_quote;

-- Drop the old table
DROP TABLE melt_quote;

-- Rename the new table to the original table name
ALTER TABLE melt_quote_new RENAME TO melt_quote;
