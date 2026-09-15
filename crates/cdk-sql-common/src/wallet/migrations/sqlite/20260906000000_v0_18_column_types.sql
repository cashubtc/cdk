-- Amounts a mint sends the wallet are u64. SQLite's only integer is signed 64
-- bit and it promotes an overflow to a float rather than failing, so the top
-- half of the range had no representation here.
--
-- Amounts become their eight big endian bytes, which keeps memcmp over equal
-- length blobs the same as numeric order so equality, ordering and indexes work
-- without a cast. The u64 function the driver registers is the conversion, and
-- it refuses a negative row rather than wrapping it; totals go through u64_sum.
-- The CHECK constraints use only built ins, since a schema naming a registered
-- function could not be read by a connection that had not registered it.
--
-- Only amounts move. Timestamps, expiries, the fee rate and the local counters
-- stay integers. The int4 widening its postgres counterpart does has no
-- equivalent here, because SQLite's INTEGER is already 64 bit.
--
-- The declared type has to change, not just the stored value: only a BLOB
-- column stores the bytes untouched, where an INTEGER affinity column would
-- convert them. SQLite has no ALTER COLUMN, so each table is rebuilt.

CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 'PENDING_SPENT' ) ) NOT NULL,
    spending_condition TEXT,
    unit TEXT NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    dleq_e BLOB,
    dleq_s BLOB,
    dleq_r BLOB,
    p2pk_e BLOB,
    used_by_operation TEXT,
    created_by_operation TEXT,
    derivation_index INTEGER
);

INSERT INTO proof_new
SELECT y, mint_url, state, spending_condition, unit, u64(amount),
       keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, p2pk_e,
       used_by_operation, created_by_operation, derivation_index
FROM proof;

DROP TABLE proof;
ALTER TABLE proof_new RENAME TO proof;

CREATE INDEX proof_used_by_operation_index ON proof(used_by_operation);
CREATE INDEX proof_created_by_operation_index ON proof(created_by_operation);
CREATE INDEX proof_keyset_state_derivation_index
ON proof(keyset_id, state, derivation_index);

CREATE TABLE melt_quote_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    request TEXT NOT NULL,
    fee_reserve BLOB NOT NULL
        CHECK (typeof(fee_reserve) = 'blob' AND length(fee_reserve) = 8),
    expiry INTEGER NOT NULL,
    state TEXT CHECK ( state IN ('UNPAID', 'PENDING', 'PAID' ) ) NOT NULL DEFAULT 'UNPAID',
    payment_proof TEXT,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    used_by_operation TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    mint_url TEXT,
    estimated_blocks INTEGER,
    fee_index INTEGER
);

INSERT INTO melt_quote_new
SELECT id, unit, u64(amount), request, u64(fee_reserve),
       expiry, state, payment_proof, payment_method, used_by_operation, version,
       mint_url, estimated_blocks, fee_index
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;

CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE INDEX melt_quote_used_by_operation_index ON melt_quote(used_by_operation);

CREATE TABLE mint_quote_new (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    amount BLOB
        CHECK (amount IS NULL
               OR (typeof(amount) = 'blob' AND length(amount) = 8)),
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    state TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    amount_paid BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(amount_paid) = 'blob' AND length(amount_paid) = 8),
    amount_issued BLOB NOT NULL DEFAULT x'0000000000000000'
        CHECK (typeof(amount_issued) = 'blob' AND length(amount_issued) = 8),
    secret_key TEXT,
    used_by_operation TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    estimated_blocks INTEGER,
    updated_at INTEGER NOT NULL DEFAULT 0
);

INSERT INTO mint_quote_new
SELECT id, mint_url, payment_method,
       u64(amount),
       unit, request, state, expiry, u64(amount_paid),
       u64(amount_issued), secret_key, used_by_operation, version,
       estimated_blocks, updated_at
FROM mint_quote;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;

CREATE INDEX idx_mint_quote_pending
ON mint_quote(payment_method, amount_issued);
CREATE INDEX mint_quote_used_by_operation_index ON mint_quote(used_by_operation);

CREATE TABLE transactions_new (
    id BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    direction TEXT CHECK (direction IN ('Incoming', 'Outgoing')) NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    fee BLOB NOT NULL
        CHECK (typeof(fee) = 'blob' AND length(fee) = 8),
    unit TEXT NOT NULL,
    ys BLOB NOT NULL,
    timestamp INTEGER NOT NULL,
    memo TEXT,
    metadata TEXT,
    quote_id TEXT,
    payment_request TEXT,
    payment_proof TEXT,
    payment_method TEXT,
    saga_id TEXT,
    status TEXT NOT NULL DEFAULT 'completed'
        CHECK (status IN ('pending', 'completed', 'failed'))
);

INSERT INTO transactions_new
SELECT id, mint_url, direction, u64(amount), u64(fee),
       unit, ys, timestamp, memo, metadata, quote_id, payment_request,
       payment_proof, payment_method, saga_id, status
FROM transactions;

DROP TABLE transactions;
ALTER TABLE transactions_new RENAME TO transactions;

CREATE INDEX mint_url_index ON transactions(mint_url);
CREATE INDEX direction_index ON transactions(direction);
CREATE INDEX unit_index ON transactions(unit);
CREATE INDEX timestamp_index ON transactions(timestamp);
CREATE INDEX transactions_saga_id_index ON transactions(saga_id);

CREATE TABLE wallet_sagas_new (
    id TEXT PRIMARY KEY,
    kind TEXT CHECK (kind IN ('send', 'receive', 'swap', 'mint', 'melt')) NOT NULL,
    state TEXT NOT NULL,
    amount BLOB NOT NULL
        CHECK (typeof(amount) = 'blob' AND length(amount) = 8),
    mint_url TEXT NOT NULL,
    unit TEXT NOT NULL,
    quote_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    data TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0
);

INSERT INTO wallet_sagas_new
SELECT id, kind, state, u64(amount), mint_url, unit, quote_id,
       created_at, updated_at, data, version
FROM wallet_sagas;

DROP TABLE wallet_sagas;
ALTER TABLE wallet_sagas_new RENAME TO wallet_sagas;

CREATE INDEX wallet_sagas_mint_url_index ON wallet_sagas(mint_url);
CREATE INDEX wallet_sagas_kind_index ON wallet_sagas(kind);
CREATE INDEX wallet_sagas_created_at_index ON wallet_sagas(created_at);
