-- Point every wallet table at mint.id instead of mint.mint_url.
--
-- A mint URL is a mutable attribute of a mint, not its identity. Keying every
-- table on the URL meant a mint that moves (NUT-06 `urls`) had to have each of
-- those tables rewritten, and any table missed left rows stranded under the old
-- URL.

-- Rows could reference a mint URL that was never added to mint. Give those a
-- mint row so nothing is stranded once the reference becomes an id.
INSERT INTO mint (mint_url)
SELECT mint_url FROM (
    SELECT mint_url FROM keyset
    UNION SELECT mint_url FROM proof
    UNION SELECT mint_url FROM mint_quote
    UNION SELECT mint_url FROM melt_quote WHERE mint_url IS NOT NULL
    UNION SELECT mint_url FROM transactions
    UNION SELECT mint_url FROM wallet_sagas
) AS referenced
WHERE mint_url NOT IN (SELECT mint_url FROM mint);

-- SQLite cannot add AUTOINCREMENT to an existing table, nor drop the column
-- keyset's foreign key depends on, so both tables are rebuilt.
--
-- The order matters. keyset references mint(mint_url) ON DELETE CASCADE, and a
-- DROP TABLE with foreign keys enforced performs an implicit DELETE FROM that
-- fires that cascade, so dropping the old mint before detaching keyset would
-- delete every keyset row. Renaming the old mint aside instead repoints the
-- cascade at a table nothing needs any more, dropped last once keyset has been
-- rebuilt against the new one.
--
-- PRAGMA foreign_keys cannot be used to avoid this: SQLite ignores it inside a
-- transaction, and the migration runner always holds one.
CREATE TABLE mint_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
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
    -- A removed mint keeps its row so the proofs, quotes and history attached
    -- to it are not destroyed with it; it is hidden from every read instead.
    removed_at INTEGER
);

INSERT INTO mint_new (
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url
)
SELECT
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url
FROM mint;

ALTER TABLE mint RENAME TO mint_old;
ALTER TABLE mint_new RENAME TO mint;

-- The foreign key below restricts rather than cascades. A mint row is only
-- ever soft deleted (removed_at), so a DELETE FROM mint is a mistake, and
-- failing it loudly beats taking every proof of that mint with it.
CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    keyset_u32 INTEGER,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER,
    final_expiry INTEGER DEFAULT NULL,
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE RESTRICT
);

INSERT INTO keyset_new (id, mint_id, keyset_u32, unit, active, input_fee_ppk, final_expiry)
SELECT k.id, m.id, k.keyset_u32, k.unit, k.active, k.input_fee_ppk, k.final_expiry
FROM keyset k
JOIN mint m ON m.mint_url = k.mint_url;

DROP TABLE keyset;
ALTER TABLE keyset_new RENAME TO keyset;
DROP TABLE mint_old;

CREATE UNIQUE INDEX IF NOT EXISTS keyset_u32_unique_keyset ON keyset(keyset_u32);
CREATE INDEX IF NOT EXISTS keyset_mint_id_index ON keyset(mint_id);

-- Every remaining table is rebuilt rather than altered. SQLite has no
-- ALTER TABLE ... ADD CONSTRAINT, so a foreign key can only be declared at
-- CREATE TABLE, and ADD COLUMN cannot take the NOT NULL either. Postgres says
-- both things about these columns; this is how SQLite says them. SQLite itself
-- enforces neither foreign key unless PRAGMA foreign_keys is set, which the
-- pool does not do, so what stands guard is mint_id_for_write resolving the id
-- from a mint row before any write.
--
-- The joins below are LEFT rather than inner so that a row whose mint_url never
-- reached mint trips the NOT NULL instead of vanishing from the copy. The
-- backfill at the top of this file is what keeps that from firing.
--
-- A rebuild takes the table's indexes with it, so each one is recreated here,
-- named for its table. An index name is global to the database rather than
-- owned by a table, and the unprefixed names had already collided:
-- mint_url_index and unit_index were created on proof by
-- 20240612132920_init.sql, silently not recreated on the rebuilt proof by
-- 20250314082116_allow_pending_spent.sql because the names were still taken,
-- and so landed on transactions once 20250401120000_add_transactions_table.sql
-- claimed the freed names.

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
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE RESTRICT
);

INSERT INTO proof_new (
    y, mint_id, state, spending_condition, unit, amount, keyset_id, secret, c,
    witness, dleq_e, dleq_s, dleq_r, p2pk_e, used_by_operation,
    created_by_operation, derivation_index
)
SELECT
    p.y, m.id, p.state, p.spending_condition, p.unit, p.amount, p.keyset_id,
    p.secret, p.c, p.witness, p.dleq_e, p.dleq_s, p.dleq_r, p.p2pk_e,
    p.used_by_operation, p.created_by_operation, p.derivation_index
FROM proof p
LEFT JOIN mint m ON m.mint_url = p.mint_url;

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
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE RESTRICT
);

INSERT INTO mint_quote_new (
    id, mint_id, payment_method, amount, unit, request, state, expiry,
    amount_paid, amount_issued, secret_key, used_by_operation, version,
    estimated_blocks, updated_at
)
SELECT
    q.id, m.id, q.payment_method, q.amount, q.unit, q.request, q.state,
    q.expiry, q.amount_paid, q.amount_issued, q.secret_key,
    q.used_by_operation, q.version, q.estimated_blocks, q.updated_at
FROM mint_quote q
LEFT JOIN mint m ON m.mint_url = q.mint_url;

DROP TABLE mint_quote;
ALTER TABLE mint_quote_new RENAME TO mint_quote;

CREATE INDEX mint_quote_mint_id_index ON mint_quote(mint_id);
CREATE INDEX mint_quote_pending_index ON mint_quote(payment_method, amount_issued);
CREATE INDEX mint_quote_used_by_operation_index ON mint_quote(used_by_operation);

-- melt_quote.mint_url was nullable, so its mint_id stays nullable too.
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
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE RESTRICT
);

INSERT INTO melt_quote_new (
    id, unit, amount, request, fee_reserve, expiry, state, payment_proof,
    payment_method, used_by_operation, version, mint_id, estimated_blocks,
    fee_index
)
SELECT
    q.id, q.unit, q.amount, q.request, q.fee_reserve, q.expiry, q.state,
    q.payment_proof, q.payment_method, q.used_by_operation, q.version, m.id,
    q.estimated_blocks, q.fee_index
FROM melt_quote q
LEFT JOIN mint m ON m.mint_url = q.mint_url;

DROP TABLE melt_quote;
ALTER TABLE melt_quote_new RENAME TO melt_quote;

CREATE INDEX melt_quote_mint_id_index ON melt_quote(mint_id);
CREATE INDEX melt_quote_state_index ON melt_quote(state);
CREATE INDEX melt_quote_used_by_operation_index ON melt_quote(used_by_operation);

CREATE TABLE transactions_new (
    id BLOB PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    direction TEXT CHECK (direction IN ('Incoming', 'Outgoing')) NOT NULL,
    amount INTEGER NOT NULL,
    fee INTEGER NOT NULL,
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
    status TEXT NOT NULL DEFAULT 'completed' CHECK (status IN ('pending', 'completed', 'failed')),
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE RESTRICT
);

INSERT INTO transactions_new (
    id, mint_id, direction, amount, fee, unit, ys, timestamp, memo, metadata,
    quote_id, payment_request, payment_proof, payment_method, saga_id, status
)
SELECT
    t.id, m.id, t.direction, t.amount, t.fee, t.unit, t.ys, t.timestamp,
    t.memo, t.metadata, t.quote_id, t.payment_request, t.payment_proof,
    t.payment_method, t.saga_id, t.status
FROM transactions t
LEFT JOIN mint m ON m.mint_url = t.mint_url;

DROP TABLE transactions;
ALTER TABLE transactions_new RENAME TO transactions;

CREATE INDEX transactions_mint_id_index ON transactions(mint_id);
CREATE INDEX transactions_direction_index ON transactions(direction);
CREATE INDEX transactions_unit_index ON transactions(unit);
CREATE INDEX transactions_timestamp_index ON transactions(timestamp);
CREATE INDEX transactions_saga_id_index ON transactions(saga_id);

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
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE RESTRICT
);

INSERT INTO wallet_sagas_new (
    id, kind, state, amount, mint_id, unit, quote_id, created_at, updated_at,
    data, version
)
SELECT
    s.id, s.kind, s.state, s.amount, m.id, s.unit, s.quote_id, s.created_at,
    s.updated_at, s.data, s.version
FROM wallet_sagas s
LEFT JOIN mint m ON m.mint_url = s.mint_url;

DROP TABLE wallet_sagas;
ALTER TABLE wallet_sagas_new RENAME TO wallet_sagas;

CREATE INDEX wallet_sagas_mint_id_index ON wallet_sagas(mint_id);
CREATE INDEX wallet_sagas_kind_index ON wallet_sagas(kind);
CREATE INDEX wallet_sagas_created_at_index ON wallet_sagas(created_at);
