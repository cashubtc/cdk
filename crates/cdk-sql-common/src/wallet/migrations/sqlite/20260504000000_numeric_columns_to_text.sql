-- Migrate amount/fee INTEGER columns to TEXT
-- u64 amounts cannot be faithfully represented as SQLite INTEGER (signed i64)
-- Only amount and fee columns are migrated; timestamps and indices stay as INTEGER

-- ============================================================================
-- proof
-- ============================================================================
CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    state TEXT CHECK (state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 'PENDING_SPENT')) NOT NULL,
    spending_condition TEXT,
    unit TEXT NOT NULL,
    amount TEXT NOT NULL,
    keyset_id TEXT NOT NULL,
    secret TEXT NOT NULL,
    c BLOB NOT NULL,
    witness TEXT,
    dleq_e BLOB,
    dleq_s BLOB,
    dleq_r BLOB,
    p2pk_e BLOB,
    used_by_operation TEXT,
    created_by_operation TEXT
);

INSERT INTO proof_new (y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, p2pk_e, used_by_operation, created_by_operation)
SELECT y, mint_url, state, spending_condition, unit, CAST(amount AS TEXT), keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, p2pk_e, used_by_operation, created_by_operation
FROM proof;

DROP TABLE proof;
ALTER TABLE proof_new RENAME TO proof;

CREATE INDEX IF NOT EXISTS secret_index ON proof(secret);
CREATE INDEX IF NOT EXISTS state_index ON proof(state);
CREATE INDEX IF NOT EXISTS spending_condition_index ON proof(spending_condition);
CREATE INDEX IF NOT EXISTS unit_index ON proof(unit);
CREATE INDEX IF NOT EXISTS amount_index ON proof(amount);
CREATE INDEX IF NOT EXISTS mint_url_index ON proof(mint_url);
CREATE INDEX IF NOT EXISTS proof_used_by_operation_index ON proof(used_by_operation);
CREATE INDEX IF NOT EXISTS proof_created_by_operation_index ON proof(created_by_operation);

-- ============================================================================
-- mint_quote (only amount columns change to TEXT)
-- ============================================================================
CREATE TABLE mint_quote_new (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    amount TEXT,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    state TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    amount_paid TEXT NOT NULL DEFAULT '0',
    amount_issued TEXT NOT NULL DEFAULT '0',
    secret_key TEXT,
    created_time INTEGER NOT NULL DEFAULT 0,
    used_by_operation TEXT,
    version INTEGER NOT NULL DEFAULT 0
);

INSERT INTO mint_quote_new (id, mint_url, payment_method, amount, unit, request, state, expiry, amount_paid, amount_issued, secret_key, created_time, used_by_operation, version)
SELECT id, mint_url, payment_method, CAST(amount AS TEXT), unit, request, state, expiry, CAST(amount_paid AS TEXT), CAST(amount_issued AS TEXT), secret_key, created_time, used_by_operation, version
FROM mint_quote;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;

CREATE INDEX IF NOT EXISTS mint_quote_used_by_operation_index ON mint_quote(used_by_operation);

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
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    used_by_operation TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    mint_url TEXT
);

INSERT INTO melt_quote_new (id, unit, amount, request, fee_reserve, expiry, state, payment_proof, payment_method, used_by_operation, version, mint_url)
SELECT id, unit, CAST(amount AS TEXT), request, CAST(fee_reserve AS TEXT), expiry, state, payment_proof, payment_method, used_by_operation, version, mint_url
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;

CREATE INDEX IF NOT EXISTS melt_quote_state_index ON melt_quote(state);
CREATE INDEX IF NOT EXISTS melt_quote_used_by_operation_index ON melt_quote(used_by_operation);

-- ============================================================================
-- transactions (only amount and fee change to TEXT)
-- ============================================================================
CREATE TABLE transactions_new (
    id BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    direction TEXT CHECK (direction IN ('Incoming', 'Outgoing')) NOT NULL,
    amount TEXT NOT NULL,
    fee TEXT NOT NULL,
    unit TEXT NOT NULL,
    ys BLOB NOT NULL,
    timestamp INTEGER NOT NULL,
    memo TEXT,
    metadata TEXT,
    quote_id TEXT,
    payment_request TEXT,
    payment_proof TEXT,
    payment_method TEXT,
    saga_id TEXT
);

INSERT INTO transactions_new (id, mint_url, direction, amount, fee, unit, ys, timestamp, memo, metadata, quote_id, payment_request, payment_proof, payment_method, saga_id)
SELECT id, mint_url, direction, CAST(amount AS TEXT), CAST(fee AS TEXT), unit, ys, timestamp, memo, metadata, quote_id, payment_request, payment_proof, payment_method, saga_id
FROM transactions;

DROP TABLE transactions;
ALTER TABLE transactions_new RENAME TO transactions;

CREATE INDEX IF NOT EXISTS mint_url_index ON transactions(mint_url);
CREATE INDEX IF NOT EXISTS direction_index ON transactions(direction);
CREATE INDEX IF NOT EXISTS unit_index ON transactions(unit);
CREATE INDEX IF NOT EXISTS timestamp_index ON transactions(timestamp);
CREATE INDEX IF NOT EXISTS transactions_saga_id_index ON transactions(saga_id);

-- ============================================================================
-- wallet_sagas (only amount changes to TEXT)
-- ============================================================================
CREATE TABLE wallet_sagas_new (
    id TEXT PRIMARY KEY,
    kind TEXT CHECK (kind IN ('send', 'receive', 'swap', 'mint', 'melt')) NOT NULL,
    state TEXT NOT NULL,
    amount TEXT NOT NULL,
    mint_url TEXT NOT NULL,
    unit TEXT NOT NULL,
    quote_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    data TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0
);

INSERT INTO wallet_sagas_new (id, kind, state, amount, mint_url, unit, quote_id, created_at, updated_at, data, version)
SELECT id, kind, state, CAST(amount AS TEXT), mint_url, unit, quote_id, created_at, updated_at, data, version
FROM wallet_sagas;

DROP TABLE wallet_sagas;
ALTER TABLE wallet_sagas_new RENAME TO wallet_sagas;

CREATE INDEX IF NOT EXISTS wallet_sagas_mint_url_index ON wallet_sagas(mint_url);
CREATE INDEX IF NOT EXISTS wallet_sagas_kind_index ON wallet_sagas(kind);
CREATE INDEX IF NOT EXISTS wallet_sagas_created_at_index ON wallet_sagas(created_at);
