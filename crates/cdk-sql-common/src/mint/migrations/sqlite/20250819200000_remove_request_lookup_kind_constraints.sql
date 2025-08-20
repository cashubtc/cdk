
-- Set existing NULL or empty request_lookup_id_kind values to 'payment_hash' in melt_quote  
UPDATE melt_quote SET request_lookup_id_kind = 'payment_hash' WHERE request_lookup_id_kind IS NULL OR request_lookup_id_kind = '';

-- Remove NOT NULL constraint and default value from request_lookup_id_kind in melt_quote table
CREATE TABLE melt_quote_temp (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    request TEXT NOT NULL,
    fee_reserve INTEGER NOT NULL,
    expiry INTEGER NOT NULL,
    state TEXT CHECK (
        state IN ('UNPAID', 'PENDING', 'PAID')
    ) NOT NULL DEFAULT 'UNPAID',
    payment_preimage TEXT,
    request_lookup_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    paid_time INTEGER,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    options TEXT,
    request_lookup_id_kind TEXT
);

INSERT INTO melt_quote_temp (id, unit, amount, request, fee_reserve, expiry, state, payment_preimage, request_lookup_id, created_time, paid_time, payment_method, options, request_lookup_id_kind) 
SELECT id, unit, amount, request, fee_reserve, expiry, state, payment_preimage, request_lookup_id, created_time, paid_time, payment_method, options, request_lookup_id_kind 
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_temp RENAME TO melt_quote;

-- Recreate indexes for melt_quote
CREATE INDEX IF NOT EXISTS melt_quote_state_index ON melt_quote(state);
CREATE UNIQUE INDEX IF NOT EXISTS unique_request_lookup_id_melt ON melt_quote(request_lookup_id);
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id_and_kind ON melt_quote(request_lookup_id, request_lookup_id_kind);
