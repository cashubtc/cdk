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

CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    keyset_u32 INTEGER,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER,
    final_expiry INTEGER DEFAULT NULL,
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE CASCADE
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

-- SQLite refuses to drop an indexed column, so every index over a mint_url goes
-- first. An index name is global to the database rather than owned by a table:
-- mint_url_index is created on proof by 20240612132920_init.sql and recreated
-- there by 20250314082116_allow_pending_spent.sql, while
-- 20250401120000_add_transactions_table.sql re-declares it IF NOT EXISTS and so
-- never moves it to transactions.
DROP INDEX IF EXISTS mint_url_index;
DROP INDEX IF EXISTS wallet_sagas_mint_url_index;

ALTER TABLE proof ADD COLUMN mint_id INTEGER;
UPDATE proof SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = proof.mint_url);
ALTER TABLE proof DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS proof_mint_id_index ON proof(mint_id);

ALTER TABLE mint_quote ADD COLUMN mint_id INTEGER;
UPDATE mint_quote SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = mint_quote.mint_url);
ALTER TABLE mint_quote DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS mint_quote_mint_id_index ON mint_quote(mint_id);

-- melt_quote.mint_url was nullable, so its mint_id stays nullable too.
ALTER TABLE melt_quote ADD COLUMN mint_id INTEGER;
UPDATE melt_quote SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = melt_quote.mint_url);
ALTER TABLE melt_quote DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS melt_quote_mint_id_index ON melt_quote(mint_id);

ALTER TABLE transactions ADD COLUMN mint_id INTEGER;
UPDATE transactions SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = transactions.mint_url);
ALTER TABLE transactions DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS transactions_mint_id_index ON transactions(mint_id);

ALTER TABLE wallet_sagas ADD COLUMN mint_id INTEGER;
UPDATE wallet_sagas SET mint_id = (SELECT m.id FROM mint m WHERE m.mint_url = wallet_sagas.mint_url);
ALTER TABLE wallet_sagas DROP COLUMN mint_url;
CREATE INDEX IF NOT EXISTS wallet_sagas_mint_id_index ON wallet_sagas(mint_id);
