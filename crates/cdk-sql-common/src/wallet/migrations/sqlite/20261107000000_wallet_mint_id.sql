CREATE TABLE mint_new (
    mint_id INTEGER PRIMARY KEY,
    mint_url TEXT NOT NULL UNIQUE,
    name TEXT,
    pubkey BLOB,
    version TEXT,
    description TEXT,
    description_long TEXT,
    contact TEXT,
    nuts TEXT,
    motd TEXT,
    icon_url TEXT,
    mint_time INTEGER,
    urls TEXT,
    tos_url TEXT,
    registered BOOLEAN NOT NULL DEFAULT FALSE
);

INSERT INTO mint_new (
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url, registered
)
SELECT
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url, TRUE
FROM mint;

INSERT INTO mint_new (mint_url)
SELECT mint_url FROM (
    SELECT mint_url FROM keyset
    UNION
    SELECT mint_url FROM proof
    UNION
    SELECT mint_url FROM mint_quote
    UNION
    SELECT mint_url FROM melt_quote
    UNION
    SELECT mint_url FROM transactions
    UNION
    SELECT mint_url FROM wallet_sagas
) AS referenced
WHERE mint_url IS NOT NULL
  AND mint_url NOT IN (SELECT mint_url FROM mint_new);

DROP TABLE mint;
ALTER TABLE mint_new RENAME TO mint;

CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    keyset_u32 INTEGER,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER,
    final_expiry INTEGER DEFAULT NULL,
    FOREIGN KEY(mint_id) REFERENCES mint(mint_id) ON DELETE CASCADE
);

INSERT INTO keyset_new (id, mint_id, keyset_u32, unit, active, input_fee_ppk, final_expiry)
SELECT
    keyset.id,
    (SELECT mint.mint_id FROM mint WHERE mint.mint_url = keyset.mint_url),
    keyset.keyset_u32,
    keyset.unit,
    keyset.active,
    keyset.input_fee_ppk,
    keyset.final_expiry
FROM keyset;

DROP TABLE keyset;
ALTER TABLE keyset_new RENAME TO keyset;
CREATE INDEX keyset_mint_id_index ON keyset(mint_id);

CREATE TABLE proof_new (
    y BLOB PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    state TEXT CHECK ( state IN ('SPENT', 'UNSPENT', 'PENDING', 'RESERVED', 'PENDING_SPENT' ) ) NOT NULL,
    spending_condition TEXT,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
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
    derivation_index INTEGER,
    FOREIGN KEY(mint_id) REFERENCES mint(mint_id)
);

INSERT INTO proof_new (
    y, mint_id, state, spending_condition, unit, amount, keyset_id, secret, c,
    witness, dleq_e, dleq_s, dleq_r, p2pk_e, used_by_operation,
    created_by_operation, derivation_index
)
SELECT
    proof.y,
    (SELECT mint.mint_id FROM mint WHERE mint.mint_url = proof.mint_url),
    proof.state, proof.spending_condition, proof.unit, proof.amount,
    proof.keyset_id, proof.secret, proof.c, proof.witness,
    proof.dleq_e, proof.dleq_s, proof.dleq_r, proof.p2pk_e,
    proof.used_by_operation, proof.created_by_operation, proof.derivation_index
FROM proof;

DROP TABLE proof;
ALTER TABLE proof_new RENAME TO proof;
CREATE INDEX proof_mint_id_index ON proof(mint_id);
CREATE INDEX proof_used_by_operation_index ON proof(used_by_operation);
CREATE INDEX proof_created_by_operation_index ON proof(created_by_operation);
CREATE INDEX proof_keyset_state_derivation_index ON proof(keyset_id, state, derivation_index);

CREATE TABLE mint_quote_new (
    id TEXT PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    amount INTEGER,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    state TEXT NOT NULL,
    expiry INTEGER NOT NULL,
    amount_paid INTEGER NOT NULL DEFAULT 0,
    amount_issued INTEGER NOT NULL DEFAULT 0,
    secret_key TEXT,
    used_by_operation TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    estimated_blocks INTEGER,
    updated_at INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(mint_id) REFERENCES mint(mint_id)
);

INSERT INTO mint_quote_new (
    id, mint_id, payment_method, amount, unit, request, state, expiry,
    amount_paid, amount_issued, secret_key, used_by_operation, version,
    estimated_blocks, updated_at
)
SELECT
    mint_quote.id,
    (SELECT mint.mint_id FROM mint WHERE mint.mint_url = mint_quote.mint_url),
    mint_quote.payment_method, mint_quote.amount, mint_quote.unit,
    mint_quote.request, mint_quote.state, mint_quote.expiry,
    mint_quote.amount_paid, mint_quote.amount_issued, mint_quote.secret_key,
    mint_quote.used_by_operation, mint_quote.version,
    mint_quote.estimated_blocks, mint_quote.updated_at
FROM mint_quote;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;
CREATE INDEX mint_quote_mint_id_index ON mint_quote(mint_id);
CREATE INDEX idx_mint_quote_pending ON mint_quote(payment_method, amount_issued);
CREATE INDEX mint_quote_used_by_operation_index ON mint_quote(used_by_operation);

CREATE TABLE melt_quote_new (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    request TEXT NOT NULL,
    fee_reserve INTEGER NOT NULL,
    expiry INTEGER NOT NULL,
    state TEXT CHECK ( state IN ('UNPAID', 'PENDING', 'PAID' ) ) NOT NULL DEFAULT 'UNPAID',
    payment_proof TEXT,
    payment_method TEXT NOT NULL DEFAULT 'bolt11',
    used_by_operation TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    mint_id INTEGER,
    estimated_blocks INTEGER,
    fee_index INTEGER,
    FOREIGN KEY(mint_id) REFERENCES mint(mint_id)
);

INSERT INTO melt_quote_new (
    id, unit, amount, request, fee_reserve, expiry, state, payment_proof,
    payment_method, used_by_operation, version, mint_id, estimated_blocks, fee_index
)
SELECT
    melt_quote.id, melt_quote.unit, melt_quote.amount, melt_quote.request,
    melt_quote.fee_reserve, melt_quote.expiry, melt_quote.state,
    melt_quote.payment_proof, melt_quote.payment_method,
    melt_quote.used_by_operation, melt_quote.version,
    (SELECT mint.mint_id FROM mint WHERE mint.mint_url = melt_quote.mint_url),
    melt_quote.estimated_blocks, melt_quote.fee_index
FROM melt_quote;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;
CREATE INDEX melt_quote_mint_id_index ON melt_quote(mint_id);
CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE INDEX melt_quote_used_by_operation_index ON melt_quote(used_by_operation);

-- Dropping transactions cascades into transaction_ys, so park its rows in a
-- constraint-free copy and put them back once the parent exists again.
CREATE TABLE transaction_ys_backup AS
SELECT transaction_id, position, y FROM transaction_ys;

CREATE TABLE transactions_new (
    id BLOB PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    direction TEXT CHECK (direction IN ('Incoming', 'Outgoing')) NOT NULL,
    amount INTEGER NOT NULL,
    fee INTEGER NOT NULL,
    unit TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    memo TEXT,
    metadata TEXT,
    quote_id TEXT,
    payment_request TEXT,
    payment_proof TEXT,
    payment_method TEXT,
    saga_id TEXT,
    status TEXT NOT NULL DEFAULT 'completed' CHECK (status IN ('pending', 'completed', 'failed')),
    FOREIGN KEY(mint_id) REFERENCES mint(mint_id)
);

INSERT INTO transactions_new (
    id, mint_id, direction, amount, fee, unit, timestamp, memo, metadata,
    quote_id, payment_request, payment_proof, payment_method, saga_id, status
)
SELECT
    transactions.id,
    (SELECT mint.mint_id FROM mint WHERE mint.mint_url = transactions.mint_url),
    transactions.direction, transactions.amount, transactions.fee,
    transactions.unit, transactions.timestamp,
    transactions.memo, transactions.metadata, transactions.quote_id,
    transactions.payment_request, transactions.payment_proof,
    transactions.payment_method, transactions.saga_id, transactions.status
FROM transactions;

DROP TABLE transactions;
ALTER TABLE transactions_new RENAME TO transactions;
CREATE INDEX transactions_mint_id_index ON transactions(mint_id);
CREATE INDEX transactions_direction_index ON transactions(direction);
CREATE INDEX transactions_unit_index ON transactions(unit);
CREATE INDEX transactions_timestamp_index ON transactions(timestamp);
CREATE INDEX transactions_saga_id_index ON transactions(saga_id);

DELETE FROM transaction_ys;
INSERT INTO transaction_ys (transaction_id, position, y)
SELECT transaction_id, position, y FROM transaction_ys_backup;
DROP TABLE transaction_ys_backup;

CREATE TABLE wallet_sagas_new (
    id TEXT PRIMARY KEY,
    kind TEXT CHECK (kind IN ('send', 'receive', 'swap', 'mint', 'melt')) NOT NULL,
    state TEXT NOT NULL,
    amount INTEGER NOT NULL,
    mint_id INTEGER NOT NULL,
    unit TEXT NOT NULL,
    quote_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    data TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(mint_id) REFERENCES mint(mint_id)
);

INSERT INTO wallet_sagas_new (
    id, kind, state, amount, mint_id, unit, quote_id, created_at, updated_at,
    data, version
)
SELECT
    wallet_sagas.id, wallet_sagas.kind, wallet_sagas.state, wallet_sagas.amount,
    (SELECT mint.mint_id FROM mint WHERE mint.mint_url = wallet_sagas.mint_url),
    wallet_sagas.unit, wallet_sagas.quote_id, wallet_sagas.created_at,
    wallet_sagas.updated_at, wallet_sagas.data, wallet_sagas.version
FROM wallet_sagas;

DROP TABLE wallet_sagas;
ALTER TABLE wallet_sagas_new RENAME TO wallet_sagas;
CREATE INDEX wallet_sagas_mint_id_index ON wallet_sagas(mint_id);
CREATE INDEX wallet_sagas_kind_index ON wallet_sagas(kind);
CREATE INDEX wallet_sagas_created_at_index ON wallet_sagas(created_at);
