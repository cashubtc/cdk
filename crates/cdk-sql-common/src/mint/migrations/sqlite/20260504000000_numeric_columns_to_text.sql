-- Migrate amount/fee INTEGER columns to TEXT
-- u64 amounts cannot be faithfully represented as SQLite INTEGER (signed i64)
-- Only amount and fee columns are migrated; timestamps and indices stay as INTEGER

-- ============================================================================
-- proof
-- ============================================================================
CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    amount TEXT NOT NULL,
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    state TEXT CHECK (state IN ('SPENT', 'PENDING', 'UNSPENT', 'RESERVED', 'UNKNOWN')) NOT NULL,
    quote_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    operation_kind TEXT,
    operation_id TEXT
);

INSERT INTO proof_new (y, amount, keyset_id, secret, c, witness, state, quote_id, created_time, operation_kind, operation_id)
SELECT y, CAST(amount AS TEXT), keyset_id, secret, c, witness, state, quote_id, created_time, operation_kind, operation_id
FROM proof;

DROP TABLE proof;
ALTER TABLE proof_new RENAME TO proof;

CREATE INDEX IF NOT EXISTS state_index ON proof(state);
CREATE INDEX IF NOT EXISTS secret_index ON proof(secret);
CREATE INDEX IF NOT EXISTS idx_proof_state_operation ON proof(state, operation_kind);
CREATE INDEX IF NOT EXISTS idx_proof_operation_id ON proof(operation_kind, operation_id);

-- ============================================================================
-- blind_signature
-- ============================================================================
CREATE TABLE blind_signature_new (
    blinded_message BLOB PRIMARY KEY,
    amount TEXT NOT NULL,
    keyset_id TEXT NOT NULL,
    c BLOB NULL,
    dleq_e TEXT,
    dleq_s TEXT,
    quote_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    signed_time INTEGER,
    operation_kind TEXT,
    operation_id TEXT
);

INSERT INTO blind_signature_new (blinded_message, amount, keyset_id, c, dleq_e, dleq_s, quote_id, created_time, signed_time, operation_kind, operation_id)
SELECT blinded_message, CAST(amount AS TEXT), keyset_id, c, dleq_e, dleq_s, quote_id, created_time, signed_time, operation_kind, operation_id
FROM blind_signature;

DROP TABLE blind_signature;
ALTER TABLE blind_signature_new RENAME TO blind_signature;

CREATE INDEX IF NOT EXISTS keyset_id_index ON blind_signature(keyset_id);
CREATE INDEX IF NOT EXISTS blind_signature_quote_id_index ON blind_signature(quote_id);
CREATE INDEX IF NOT EXISTS idx_blind_sig_operation_id ON blind_signature(operation_kind, operation_id);

-- ============================================================================
-- mint_quote (only amount columns change to TEXT)
-- ============================================================================
CREATE TABLE mint_quote_new (
    id TEXT PRIMARY KEY,
    amount TEXT,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    request_lookup_id TEXT UNIQUE,
    pubkey TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    amount_paid TEXT NOT NULL DEFAULT '0',
    amount_issued TEXT NOT NULL DEFAULT '0',
    payment_method TEXT NOT NULL DEFAULT 'BOLT11',
    request_lookup_id_kind TEXT NOT NULL DEFAULT 'payment_hash',
    extra_json TEXT
);

INSERT INTO mint_quote_new (id, amount, unit, request, expiry, request_lookup_id, pubkey, created_time, amount_paid, amount_issued, payment_method, request_lookup_id_kind, extra_json)
SELECT id, CAST(amount AS TEXT), unit, request, expiry, request_lookup_id, pubkey, created_time, CAST(amount_paid AS TEXT), CAST(amount_issued AS TEXT), payment_method, request_lookup_id_kind, extra_json
FROM mint_quote;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;

CREATE INDEX IF NOT EXISTS idx_mint_quote_created_time ON mint_quote(created_time);
CREATE INDEX IF NOT EXISTS idx_mint_quote_expiry ON mint_quote(expiry);
CREATE INDEX IF NOT EXISTS idx_mint_quote_request_lookup_id ON mint_quote(request_lookup_id);
CREATE INDEX IF NOT EXISTS idx_mint_quote_request_lookup_id_and_kind ON mint_quote(request_lookup_id, request_lookup_id_kind);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mint_quote_request_unique ON mint_quote(request);

-- ============================================================================
-- mint_quote_payments (only amount changes to TEXT)
-- ============================================================================
CREATE TABLE mint_quote_payments_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    quote_id TEXT NOT NULL,
    payment_id TEXT NOT NULL UNIQUE,
    timestamp INTEGER NOT NULL,
    amount TEXT NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);

INSERT INTO mint_quote_payments_new (id, quote_id, payment_id, timestamp, amount)
SELECT id, quote_id, payment_id, timestamp, CAST(amount AS TEXT)
FROM mint_quote_payments;

DROP TABLE mint_quote_payments;
ALTER TABLE mint_quote_payments_new RENAME TO mint_quote_payments;

CREATE INDEX IF NOT EXISTS idx_mint_quote_payments_payment_id ON mint_quote_payments(payment_id);
CREATE INDEX IF NOT EXISTS idx_mint_quote_payments_quote_id ON mint_quote_payments(quote_id);

-- ============================================================================
-- mint_quote_issued (only amount changes to TEXT)
-- ============================================================================
CREATE TABLE mint_quote_issued_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    quote_id TEXT NOT NULL,
    amount TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);

INSERT INTO mint_quote_issued_new (id, quote_id, amount, timestamp)
SELECT id, quote_id, CAST(amount AS TEXT), timestamp
FROM mint_quote_issued;

DROP TABLE mint_quote_issued;
ALTER TABLE mint_quote_issued_new RENAME TO mint_quote_issued;

CREATE INDEX IF NOT EXISTS idx_mint_quote_issued_quote_id ON mint_quote_issued(quote_id);

-- ============================================================================
-- melt_quote (only amount and fee_reserve change to TEXT)
-- ============================================================================
CREATE TABLE melt_quote_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount TEXT NOT NULL,
    request TEXT NOT NULL,
    fee_reserve TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    state TEXT CHECK (state IN ('UNPAID', 'PENDING', 'PAID')) NOT NULL DEFAULT 'UNPAID',
    payment_proof TEXT,
    request_lookup_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    paid_time INTEGER,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    options TEXT,
    request_lookup_id_kind TEXT,
    extra_json TEXT
);

INSERT INTO melt_quote_new (id, unit, amount, request, fee_reserve, expiry, state, payment_proof, request_lookup_id, created_time, paid_time, payment_method, options, request_lookup_id_kind, extra_json)
SELECT id, unit, CAST(amount AS TEXT), request, CAST(fee_reserve AS TEXT), expiry, state, payment_proof, request_lookup_id, created_time, paid_time, payment_method, options, request_lookup_id_kind, extra_json
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;

CREATE INDEX IF NOT EXISTS melt_quote_state_index ON melt_quote(state);
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id ON melt_quote(request_lookup_id);
CREATE INDEX IF NOT EXISTS idx_melt_quote_request_lookup_id_and_kind ON melt_quote(request_lookup_id, request_lookup_id_kind);
CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_paid_lookup_id ON melt_quote(request_lookup_id) WHERE state IN ('PENDING', 'PAID');

-- ============================================================================
-- melt_request
-- ============================================================================
CREATE TABLE melt_request_new (
    quote_id TEXT PRIMARY KEY,
    inputs_amount TEXT NOT NULL,
    inputs_fee TEXT NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES melt_quote(id)
);

INSERT INTO melt_request_new (quote_id, inputs_amount, inputs_fee)
SELECT quote_id, CAST(inputs_amount AS TEXT), CAST(inputs_fee AS TEXT)
FROM melt_request;

DROP TABLE melt_request;
ALTER TABLE melt_request_new RENAME TO melt_request;

-- ============================================================================
-- keyset_amounts
-- ============================================================================
CREATE TABLE keyset_amounts_new (
    keyset_id TEXT PRIMARY KEY,
    total_issued TEXT NOT NULL DEFAULT '0',
    total_redeemed TEXT NOT NULL DEFAULT '0',
    fee_collected TEXT NOT NULL DEFAULT '0'
);

INSERT INTO keyset_amounts_new (keyset_id, total_issued, total_redeemed, fee_collected)
SELECT keyset_id, CAST(total_issued AS TEXT), CAST(total_redeemed AS TEXT), CAST(fee_collected AS TEXT)
FROM keyset_amounts;

DROP TABLE keyset_amounts;
ALTER TABLE keyset_amounts_new RENAME TO keyset_amounts;

-- ============================================================================
-- completed_operations (only amount columns change to TEXT)
-- ============================================================================
CREATE TABLE completed_operations_new (
    operation_id TEXT PRIMARY KEY,
    operation_kind TEXT NOT NULL,
    completed_at INTEGER NOT NULL,
    total_issued TEXT NOT NULL,
    total_redeemed TEXT NOT NULL,
    fee_collected TEXT NOT NULL,
    payment_amount TEXT,
    payment_fee TEXT,
    payment_method TEXT
);

INSERT INTO completed_operations_new (operation_id, operation_kind, completed_at, total_issued, total_redeemed, fee_collected, payment_amount, payment_fee, payment_method)
SELECT operation_id, operation_kind, completed_at, CAST(total_issued AS TEXT), CAST(total_redeemed AS TEXT), CAST(fee_collected AS TEXT), CAST(payment_amount AS TEXT), CAST(payment_fee AS TEXT), payment_method
FROM completed_operations;

DROP TABLE completed_operations;
ALTER TABLE completed_operations_new RENAME TO completed_operations;
