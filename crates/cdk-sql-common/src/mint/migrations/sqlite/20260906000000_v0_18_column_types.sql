-- Amounts are u64 on the wire. SQLite's only integer is signed 64 bit, and it
-- promotes an overflow to a float rather than failing, so the top half of the
-- range had no representation here at all.
--
-- Amounts become their eight big endian bytes. Byte order is what keeps memcmp
-- over equal length blobs the same as numeric order, so equality, ordering and
-- indexes work without a cast, and eight bytes hold the u64 range and nothing
-- else. The u64 function the driver registers is the conversion, and it refuses
-- a negative row rather than wrapping it; addition goes through u64_add and
-- u64_sum, which refuse an overflow. The CHECK constraints use only built ins,
-- since a schema naming a registered function could not be read by a connection
-- that had not registered it.
--
-- Only amounts move. Timestamps, expiries and the fee rate stay integers: a
-- clock cannot reach the signed maximum, and they still need native arithmetic
-- (mint_quote.updated_at) and range predicates (mint_quote.last_checked). The
-- int4 widening its postgres counterpart does has no equivalent here, because
-- SQLite's INTEGER is already 64 bit.
--
-- The declared type has to change, not just the stored value. Only a BLOB
-- column stores the bytes untouched; an INTEGER affinity column would convert
-- them, which is the loss this migration exists to stop. SQLite has no ALTER
-- COLUMN, so each table is rebuilt.

CREATE TABLE melt_quote_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    request TEXT NOT NULL,
    fee_reserve BLOB NOT NULL
        CHECK (typeof(fee_reserve) = 'blob' AND length(fee_reserve) = 8),
    expiry INTEGER NOT NULL,
    state TEXT CHECK (
        state IN ('UNPAID', 'PENDING', 'PAID')
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

INSERT INTO melt_quote_new
SELECT id, unit, u64(amount), request, u64(fee_reserve),
       expiry, state, payment_proof, request_lookup_id, created_time, paid_time,
       payment_method, options, request_lookup_id_kind, estimated_blocks,
       fee_options, extra_json, selected_fee_index
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;

CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE INDEX idx_melt_quote_request_lookup_id ON melt_quote(request_lookup_id);
CREATE UNIQUE INDEX unique_pending_paid_lookup_id
ON melt_quote(request_lookup_id)
WHERE state IN ('PENDING', 'PAID');
CREATE INDEX idx_melt_quote_request_lookup_id_and_kind
ON melt_quote(request_lookup_id, request_lookup_id_kind);

CREATE TABLE melt_request_new (
    quote_id TEXT PRIMARY KEY,
    inputs_amount BLOB NOT NULL
        CHECK (typeof(inputs_amount) = 'blob' AND length(inputs_amount) = 8),
    inputs_fee BLOB NOT NULL
        CHECK (typeof(inputs_fee) = 'blob' AND length(inputs_fee) = 8),
    FOREIGN KEY (quote_id) REFERENCES melt_quote(id)
);

INSERT INTO melt_request_new
SELECT quote_id, u64(inputs_amount), u64(inputs_fee)
FROM melt_request;

DROP TABLE melt_request;
ALTER TABLE melt_request_new RENAME TO melt_request;

CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
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

INSERT INTO proof_new
SELECT y, u64(amount), keyset_id, secret, c, witness, state,
       quote_id, created_time, operation_kind, operation_id
FROM proof;

DROP TABLE proof;
ALTER TABLE proof_new RENAME TO proof;

CREATE INDEX idx_proof_state_operation ON proof(state, operation_kind);
CREATE INDEX idx_proof_operation_id ON proof(operation_kind, operation_id);

CREATE TABLE blind_signature_new (
    blinded_message BLOB PRIMARY KEY,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    keyset_id TEXT NOT NULL,
    c BLOB NULL,
    dleq_e TEXT,
    dleq_s TEXT,
    quote_id TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    signed_time INTEGER,
    operation_kind TEXT,
    operation_id TEXT,
    order_index INTEGER DEFAULT 0
);

INSERT INTO blind_signature_new
SELECT blinded_message, u64(amount), keyset_id, c, dleq_e, dleq_s,
       quote_id, created_time, signed_time, operation_kind, operation_id,
       order_index
FROM blind_signature;

DROP TABLE blind_signature;
ALTER TABLE blind_signature_new RENAME TO blind_signature;

CREATE INDEX keyset_id_index ON blind_signature(keyset_id);
CREATE INDEX blind_signature_quote_id_index ON blind_signature(quote_id);
CREATE INDEX idx_blind_sig_operation_id ON blind_signature(operation_kind, operation_id);

CREATE TABLE mint_quote_new (
    id TEXT PRIMARY KEY,
    amount BLOB
        CHECK (amount IS NULL
               OR (typeof(amount) = 'blob' AND length(amount) = 8)),
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    request_lookup_id TEXT UNIQUE,
    pubkey TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    amount_paid BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(amount_paid) = 'blob' AND length(amount_paid) = 8),
    amount_issued BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(amount_issued) = 'blob' AND length(amount_issued) = 8),
    payment_method TEXT NOT NULL DEFAULT 'BOLT11',
    request_lookup_id_kind TEXT NOT NULL DEFAULT 'payment_hash',
    extra_json TEXT,
    updated_at INTEGER NOT NULL DEFAULT 0,
    last_checked INTEGER NOT NULL DEFAULT 0
);

INSERT INTO mint_quote_new
SELECT id,
       u64(amount),
       unit, request, expiry, request_lookup_id, pubkey, created_time,
       u64(amount_paid), u64(amount_issued),
       payment_method, request_lookup_id_kind, extra_json, updated_at,
       last_checked
FROM mint_quote;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;

CREATE INDEX idx_mint_quote_created_time ON mint_quote(created_time);
CREATE INDEX idx_mint_quote_expiry ON mint_quote(expiry);
CREATE INDEX idx_mint_quote_request_lookup_id ON mint_quote(request_lookup_id);
CREATE INDEX idx_mint_quote_request_lookup_id_and_kind ON mint_quote(request_lookup_id, request_lookup_id_kind);
CREATE UNIQUE INDEX idx_mint_quote_request_unique ON mint_quote(request);

CREATE TABLE mint_quote_payments_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    quote_id TEXT NOT NULL,
    payment_id TEXT NOT NULL UNIQUE,
    timestamp INTEGER NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);

INSERT INTO mint_quote_payments_new
SELECT id, quote_id, payment_id, timestamp, u64(amount)
FROM mint_quote_payments;

DROP TABLE mint_quote_payments;
ALTER TABLE mint_quote_payments_new RENAME TO mint_quote_payments;

CREATE INDEX idx_mint_quote_payments_payment_id ON mint_quote_payments(payment_id);
CREATE INDEX idx_mint_quote_payments_quote_id ON mint_quote_payments(quote_id);

CREATE TABLE mint_quote_issued_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    quote_id TEXT NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    timestamp INTEGER NOT NULL,
    FOREIGN KEY (quote_id) REFERENCES mint_quote(id)
);

INSERT INTO mint_quote_issued_new
SELECT id, quote_id, u64(amount), timestamp
FROM mint_quote_issued;

DROP TABLE mint_quote_issued;
ALTER TABLE mint_quote_issued_new RENAME TO mint_quote_issued;

CREATE INDEX idx_mint_quote_issued_quote_id ON mint_quote_issued(quote_id);

CREATE TABLE keyset_amounts_new (
    keyset_id TEXT PRIMARY KEY NOT NULL,
    total_issued BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(total_issued) = 'blob' AND length(total_issued) = 8),
    total_redeemed BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(total_redeemed) = 'blob' AND length(total_redeemed) = 8),
    fee_collected BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(fee_collected) = 'blob' AND length(fee_collected) = 8)
);

INSERT INTO keyset_amounts_new
SELECT keyset_id, u64(total_issued), u64(total_redeemed),
       u64(fee_collected)
FROM keyset_amounts;

DROP TABLE keyset_amounts;
ALTER TABLE keyset_amounts_new RENAME TO keyset_amounts;

CREATE TABLE completed_operations_new (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_kind TEXT NOT NULL,
    completed_at INTEGER NOT NULL,
    total_issued BLOB NOT NULL
        CHECK (typeof(total_issued) = 'blob' AND length(total_issued) = 8),
    total_redeemed BLOB NOT NULL
        CHECK (typeof(total_redeemed) = 'blob' AND length(total_redeemed) = 8),
    fee_collected BLOB NOT NULL
        CHECK (typeof(fee_collected) = 'blob' AND length(fee_collected) = 8),
    payment_amount BLOB
        CHECK (payment_amount IS NULL
               OR (typeof(payment_amount) = 'blob' AND length(payment_amount) = 8)),
    payment_fee BLOB
        CHECK (payment_fee IS NULL
               OR (typeof(payment_fee) = 'blob' AND length(payment_fee) = 8)),
    payment_method TEXT
);

INSERT INTO completed_operations_new
SELECT operation_id, operation_kind, completed_at, u64(total_issued),
       u64(total_redeemed), u64(fee_collected),
       u64(payment_amount),
       u64(payment_fee),
       payment_method
FROM completed_operations;

DROP TABLE completed_operations;
ALTER TABLE completed_operations_new RENAME TO completed_operations;

CREATE INDEX idx_completed_operations_kind_time ON completed_operations(operation_kind, completed_at);
CREATE INDEX idx_completed_operations_time ON completed_operations(completed_at);
