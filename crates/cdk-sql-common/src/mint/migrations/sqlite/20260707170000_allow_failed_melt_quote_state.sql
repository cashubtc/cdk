DROP INDEX IF EXISTS melt_quote_state_index;
DROP INDEX IF EXISTS request_index;
DROP INDEX IF EXISTS expiry_index;
DROP INDEX IF EXISTS unique_request_lookup_id_melt;
DROP INDEX IF EXISTS idx_melt_quote_request_lookup_id;
DROP INDEX IF EXISTS idx_melt_quote_request_lookup_id_and_kind;
DROP INDEX IF EXISTS unique_pending_paid_lookup_id;

-- melt_request has a foreign key to melt_quote. Preserve its rows explicitly
-- so the parent table can be rebuilt without disabling foreign-key checks.
CREATE TEMP TABLE melt_request_failed_state_backup AS
SELECT quote_id, inputs_amount, inputs_fee
FROM melt_request;

DELETE FROM melt_request;

CREATE TABLE melt_quote_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    request TEXT NOT NULL,
    fee_reserve INTEGER NOT NULL,
    expiry INTEGER NOT NULL,
    state TEXT CHECK (
        state IN ('UNPAID', 'PENDING', 'PAID', 'FAILED')
    ) NOT NULL DEFAULT 'UNPAID',
    payment_proof TEXT,
    request_lookup_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    paid_time INTEGER,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    options TEXT,
    request_lookup_id_kind TEXT,
    estimated_blocks INTEGER,
    fee_options TEXT,
    extra_json TEXT,
    selected_fee_index INTEGER
);

INSERT INTO melt_quote_new (
    id,
    unit,
    amount,
    request,
    fee_reserve,
    expiry,
    state,
    payment_proof,
    request_lookup_id,
    created_time,
    paid_time,
    payment_method,
    options,
    request_lookup_id_kind,
    estimated_blocks,
    fee_options,
    extra_json,
    selected_fee_index
)
SELECT
    id,
    unit,
    amount,
    request,
    fee_reserve,
    expiry,
    state,
    payment_proof,
    request_lookup_id,
    created_time,
    paid_time,
    payment_method,
    options,
    request_lookup_id_kind,
    estimated_blocks,
    fee_options,
    extra_json,
    selected_fee_index
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;

INSERT INTO melt_request (quote_id, inputs_amount, inputs_fee)
SELECT quote_id, inputs_amount, inputs_fee
FROM melt_request_failed_state_backup;

DROP TABLE melt_request_failed_state_backup;

CREATE INDEX IF NOT EXISTS melt_quote_state_index ON melt_quote(state);
CREATE INDEX IF NOT EXISTS request_index ON melt_quote(request);
CREATE INDEX IF NOT EXISTS expiry_index ON melt_quote(expiry);
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id ON melt_quote(request_lookup_id);
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id_and_kind
ON melt_quote(request_lookup_id, request_lookup_id_kind);
CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_paid_lookup_id
ON melt_quote(request_lookup_id)
WHERE state IN ('PENDING', 'PAID');
